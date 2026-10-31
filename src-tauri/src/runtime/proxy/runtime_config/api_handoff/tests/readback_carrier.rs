//! Exercise the production captured channel and readback opening, not a pending
//! stand-in. This peer speaks the existing protobuf/tonic loopback protocol.
use super::*;
use crate::commands::speedtest::StreamWitness;
use futures::StreamExt;
use polaris_singbox_grpc::tonic::{self, codegen::http, Request, Response, Status};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tonic::codegen::Service as _;

struct PeerState {
    old_bearers: AtomicUsize,
    opening: Notify,
    opening_gate: tokio::sync::Semaphore,
    stream_dropped: Notify,
    sockets: Mutex<Vec<std::net::TcpStream>>,
}
impl Default for PeerState {
    fn default() -> Self {
        Self {
            old_bearers: AtomicUsize::new(0),
            opening: Notify::new(),
            opening_gate: tokio::sync::Semaphore::new(0),
            stream_dropped: Notify::new(),
            sockets: Mutex::new(Vec::new()),
        }
    }
}

struct StreamExit(Arc<PeerState>);
impl Drop for StreamExit {
    fn drop(&mut self) {
        self.0.stream_dropped.notify_one();
    }
}

#[derive(Clone)]
struct EventService(Arc<PeerState>);
impl tonic::server::ServerStreamingService<daemon::SubscribeConnectionsRequest> for EventService {
    type Response = daemon::ConnectionEvents;
    type ResponseStream =
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<Self::Response, Status>> + Send>>;
    type Future = tonic::codegen::BoxFuture<Response<Self::ResponseStream>, Status>;
    fn call(&mut self, _: Request<daemon::SubscribeConnectionsRequest>) -> Self::Future {
        let state = self.0.clone();
        Box::pin(async move {
            state.opening.notify_one();
            let permit = state
                .opening_gate
                .acquire()
                .await
                .map_err(|_| Status::cancelled("peer stopped"))?;
            permit.forget();
            let (send, receive) = tokio::sync::mpsc::channel(1);
            send.send(Ok(daemon::ConnectionEvents::default()))
                .await
                .unwrap();
            // Retain the producer so the next real HTTP/2 body read waits for
            // cancellation, rather than returning a synthetic pending future.
            let exit = StreamExit(state);
            let stream = futures::stream::unfold(
                (send, receive, exit),
                |(send, mut receive, exit)| async move {
                    receive
                        .recv()
                        .await
                        .map(|frame| (frame, (send, receive, exit)))
                },
            );
            Ok(Response::new(Box::pin(stream) as Self::ResponseStream))
        })
    }
}

#[derive(Clone)]
struct Peer {
    runtime: daemon::runtime_config_service_server::RuntimeConfigServiceServer<ReadyApiFixture>,
    state: Arc<PeerState>,
}
impl tonic::codegen::Service<http::Request<tonic::body::Body>> for Peer {
    type Response = http::Response<tonic::body::Body>;
    type Error = std::convert::Infallible;
    type Future = tonic::codegen::BoxFuture<Self::Response, Self::Error>;
    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn call(&mut self, request: http::Request<tonic::body::Body>) -> Self::Future {
        if request
            .headers()
            .get("authorization")
            .is_some_and(|h| h == "Bearer old")
        {
            self.state.old_bearers.fetch_add(1, Ordering::SeqCst);
        }
        if request
            .uri()
            .path()
            .starts_with("/daemon.RuntimeConfigService/")
        {
            return self.runtime.call(request);
        }
        assert_eq!(
            request.uri().path(),
            "/daemon.StartedService/SubscribeConnections"
        );
        let state = self.state.clone();
        Box::pin(async move {
            let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::default());
            Ok(grpc.server_streaming(EventService(state), request).await)
        })
    }
}

struct RunningPeer {
    port: u16,
    state: Arc<PeerState>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<(), tonic::transport::Error>>>,
}
impl RunningPeer {
    async fn start(port: u16) -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(PeerState::default());
        let incoming_state = state.clone();
        let incoming = futures::stream::unfold(listener, move |listener| {
            let state = incoming_state.clone();
            async move {
                let next = listener.accept().await.and_then(|(stream, _)| {
                    let raw = stream.into_std()?;
                    state.sockets.lock().unwrap().push(raw.try_clone()?);
                    tokio::net::TcpStream::from_std(raw)
                });
                Some((next, listener))
            }
        });
        let service = Peer {
            runtime: daemon::runtime_config_service_server::RuntimeConfigServiceServer::new(
                ReadyApiFixture {
                    caps: super::super::super::tests::paired_capabilities(),
                },
            ),
            state: state.clone(),
        };
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .serve_with_incoming_shutdown(service, incoming, async {
                    let _ = stopped.await;
                })
                .await
        });
        Self {
            port,
            state,
            stop: Some(stop),
            task: Some(task),
        }
    }
    fn shutdown(&mut self) {
        self.state.opening_gate.close();
        for socket in self.state.sockets.lock().unwrap().iter() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
    async fn join(mut self) {
        self.shutdown();
        let mut task = self.task.take().unwrap();
        match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
            Ok(result) => result.unwrap().unwrap(),
            Err(_) => {
                task.abort();
                let _ = task.await;
                panic!("owned loopback peer did not join");
            }
        }
    }
}
impl Drop for RunningPeer {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn bound_runtime(
    peer: &RunningPeer,
    install_close_callback: bool,
) -> (
    Arc<ProxyRuntime>,
    Arc<ApiRelayEpoch>,
    crate::test_support::TestDir,
) {
    let (rt, _, directory) =
        ProxyRuntime::ready_api_commit_fixture_for_test(peer.port, "old".into());
    let saved = rt.config.current().unwrap();
    let typed: UserConfig = serde_json::from_value(saved.clone()).unwrap();
    let deps = rt.generate_deps(peer.port, 0, 0, None, &[], &saved, false);
    let generated =
        polaris_config_engine::builder::generate_sing_box_config(&typed, &BTreeMap::new(), &deps)
            .unwrap();
    let caps = super::super::super::tests::paired_capabilities();
    rt.capture_runtime_config_baseline(
        rt.core_generation(),
        deps,
        generated,
        typed,
        observe_runtime_pairing(&caps).unwrap(),
    );
    let control = Arc::new(
        SingBoxApiClient::connect_captured(Endpoint::new("127.0.0.1", peer.port), "old")
            .await
            .unwrap(),
    );
    control.get_runtime_capabilities().await.unwrap();
    assert!(control.captured_transport_live());
    if install_close_callback {
        rt.bind_runtime_api_control(rt.core_generation(), control);
    } else {
        // Model the interval after physical EOF and before the owner's epoch
        // invalidation callback has run. Capture itself still observes EOF.
        rt.runtime_config_baseline
            .write()
            .unwrap()
            .as_mut()
            .unwrap()
            .control = Some(control);
    }
    let epoch = rt
        .runtime_config_baseline
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .api_epoch
        .clone();
    (rt, epoch, directory)
}

#[tokio::test]
async fn readback_joins_real_captured_opening_and_stream_without_a_cached_borrow_cycle() {
    for opening in [true, false] {
        let peer = RunningPeer::start(0).await;
        let (rt, epoch, _directory) = bound_runtime(&peer, true).await;
        let verified = rt
            .runtime_config_baseline
            .write()
            .unwrap()
            .as_mut()
            .unwrap()
            .control
            .take();
        assert!(
            rt.readback_binding(None).is_none(),
            "missing verified control has no tuple fallback"
        );
        rt.runtime_config_baseline
            .write()
            .unwrap()
            .as_mut()
            .unwrap()
            .control = verified;
        let idle_binding = rt.readback_binding(None).unwrap();
        assert_eq!(epoch.state.lock().unwrap().1, 0);
        assert_eq!(
            peer.state.sockets.lock().unwrap().len(),
            1,
            "bootstrap RPC verified one physical carrier"
        );
        if !opening {
            peer.state.opening_gate.add_permits(1);
        }
        let binding = rt.readback_binding(None).unwrap();
        let body_polled = Arc::new(Notify::new());
        let body_seen = body_polled.clone();
        let witness = StreamWitness::spawn_in_api_epoch(
            async move {
                let frames = binding.open_events(0).await?;
                Ok::<_, polaris_singbox_grpc::ClientError>(Box::pin(frames.filter_map(move |_| {
                    body_seen.notify_one();
                    std::future::ready(None)
                })))
            },
            Some(epoch.clone()),
        );
        tokio::time::timeout(Duration::from_secs(5), peer.state.opening.notified())
            .await
            .unwrap();
        assert_eq!(
            peer.state.sockets.lock().unwrap().len(),
            1,
            "readback must reuse that verified carrier, not trust a second first dial"
        );
        assert_eq!(
            epoch.state.lock().unwrap().1,
            1,
            "actual opening retains its client borrower"
        );
        if !opening {
            // The opening completed and the actual response body was polled.
            tokio::time::timeout(Duration::from_secs(5), body_polled.notified())
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), epoch.seal_and_join())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(epoch.state.lock().unwrap().1, 0);
        assert!(
            epoch.tasks.lock().unwrap().is_empty(),
            "actual witness JoinHandle must have completed"
        );
        assert!(idle_binding.open_events(0).await.is_err());
        if !opening {
            tokio::time::timeout(Duration::from_secs(5), peer.state.stream_dropped.notified())
                .await
                .unwrap();
        }
        drop(witness);
        rt.clear_runtime_config_baseline();
        peer.join().await;
    }
}

#[tokio::test]
async fn delayed_readback_cannot_send_old_bearer_to_a_reoccupied_port_even_before_epoch_invalidation(
) {
    let peer = RunningPeer::start(0).await;
    let port = peer.port;
    let (rt, epoch, _directory) = bound_runtime(&peer, false).await;
    let binding = rt.readback_binding(None).unwrap();
    let control = binding.control.as_ref().unwrap().clone();
    let (release, delayed) = tokio::sync::oneshot::channel();
    let (entered, polled) = tokio::sync::oneshot::channel();
    let (result, received) = tokio::sync::oneshot::channel();
    let task = epoch
        .spawn_owned(async move {
            entered.send(()).unwrap();
            delayed.await.unwrap();
            let answer = binding.open_events(0).await;
            result.send(answer.is_err()).unwrap();
        })
        .unwrap();
    polled.await.unwrap();
    peer.join().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while control.captured_transport_live() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !epoch.is_closed(),
        "owner callback deliberately delayed, not a sealed-epoch-only test"
    );
    assert!(
        rt.readback_binding(None).is_none(),
        "dead original control must refuse new admission"
    );
    let replacement = RunningPeer::start(port).await;
    release.send(()).unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), received)
        .await
        .unwrap()
        .unwrap());
    task.join().await.unwrap();
    epoch.seal_and_join().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(replacement.state.old_bearers.load(Ordering::SeqCst), 0);
    assert!(
        replacement.state.sockets.lock().unwrap().is_empty(),
        "no replacement TCP connection, not merely a rejected RPC"
    );
    rt.clear_runtime_config_baseline();
    replacement.join().await;
}

use crate::daemon::{ConnectionEvent, ConnectionEvents};
use crate::{in_api_lifetime, ApiClientLifetime, ClientError, CONNECTION_EVENTS_MAX_FRAME_BYTES};
use futures_core::{ready, Stream};
use futures_util::{future::poll_fn, StreamExt};
use prost::Message;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tonic::{Status, Streaming};

#[derive(Debug)]
enum Reset {
    Initial,
    Collecting(usize),
    Complete,
}

/// The native daemon's limit for each frame of an opted-in snapshot.
const SNAPSHOT_FRAME_LIMIT: usize = 1024 * 1024;
const SNAPSHOT_SIZE_ERROR: &str = "connection snapshot exceeds its byte limit";
const SNAPSHOT_FRAME_SIZE_ERROR: &str = "connection snapshot frame exceeds its byte limit";
const CORE_EVENT_SIZE_ERROR: &str = "connection event exceeds stream frame limit";

impl ClientError {
    /// Whether this connection subscription hit a deterministic snapshot size limit.
    pub fn is_connection_snapshot_too_large(&self) -> bool {
        matches!(self, Self::Status(status)
            if status.code() == tonic::Code::ResourceExhausted
                && matches!(status.message(), SNAPSHOT_SIZE_ERROR | SNAPSHOT_FRAME_SIZE_ERROR | CORE_EVENT_SIZE_ERROR))
    }
}

#[derive(Debug)]
pub enum ConnectionEventFrame {
    /// An incremental frame or the complete reset from an older daemon.
    Events(ConnectionEvents),
    SnapshotPart(ConnectionEvents),
    SnapshotComplete,
}

impl Reset {
    fn accept(
        &mut self,
        frame: ConnectionEvents,
        part_limit: usize,
    ) -> Result<ConnectionEventFrame, Status> {
        match (&mut *self, frame.snapshot_complete) {
            (Self::Initial | Self::Complete, None) => {
                // An older daemon ignores the opt-in and sends a complete reset.
                *self = Self::Complete;
                Ok(ConnectionEventFrame::Events(frame))
            }
            (Self::Initial, Some(false)) if frame.reset => {
                let bytes = Self::snapshot_part_size(&frame, part_limit)?;
                *self = Self::Collecting(bytes);
                Ok(ConnectionEventFrame::SnapshotPart(frame))
            }
            (Self::Collecting(bytes), Some(false)) if !frame.reset && !frame.events.is_empty() => {
                *bytes += Self::snapshot_part_size(&frame, part_limit)?;
                if *bytes > CONNECTION_EVENTS_MAX_FRAME_BYTES {
                    return Err(Status::resource_exhausted(SNAPSHOT_SIZE_ERROR));
                }
                Ok(ConnectionEventFrame::SnapshotPart(frame))
            }
            (Self::Collecting(_), Some(true)) if !frame.reset && frame.events.is_empty() => {
                *self = Self::Complete;
                Ok(ConnectionEventFrame::SnapshotComplete)
            }
            _ => Err(Status::data_loss("invalid connection snapshot framing")),
        }
    }

    fn snapshot_part_size(frame: &ConnectionEvents, limit: usize) -> Result<usize, Status> {
        let bytes = frame.encoded_len();
        if bytes > limit {
            return Err(Status::resource_exhausted(SNAPSHOT_FRAME_SIZE_ERROR));
        }
        Ok(bytes)
    }
}

/// A single authenticated subscription. Callers can reduce parts without
/// retaining the full protobuf reset, and must discard them if this stream ends early.
pub struct ConnectionEventFrames {
    session: ValidatedFrames,
    lifetime: Option<Arc<dyn ApiClientLifetime>>,
}

impl ConnectionEventFrames {
    pub(crate) fn new(
        stream: Streaming<ConnectionEvents>,
        lifetime: Option<Arc<dyn ApiClientLifetime>>,
    ) -> Self {
        Self {
            session: ValidatedFrames::new(stream, SNAPSHOT_FRAME_LIMIT),
            lifetime,
        }
    }

    /// Dropping a pending read preserves the session. An error terminates it.
    pub async fn message(&mut self) -> Result<Option<ConnectionEventFrame>, ClientError> {
        if self.session.inner.is_none() {
            return Ok(None);
        }
        let result = in_api_lifetime(&self.lifetime, async {
            Ok(poll_fn(|cx| self.session.poll_next(cx)).await.transpose()?)
        })
        .await;
        if result.is_err() {
            self.session.inner = None;
        }
        result
    }
}

#[derive(Debug)]
struct ValidatedFrames {
    inner: Option<Streaming<ConnectionEvents>>,
    reset: Reset,
    part_limit: usize,
}

impl ValidatedFrames {
    fn new(inner: Streaming<ConnectionEvents>, part_limit: usize) -> Self {
        Self {
            inner: Some(inner),
            reset: Reset::Initial,
            part_limit,
        }
    }

    fn poll_next(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<ConnectionEventFrame, Status>>> {
        let Some(stream) = &mut self.inner else {
            return Poll::Ready(None);
        };
        let result = match ready!(stream.poll_next_unpin(cx)) {
            Some(Ok(frame)) => self.reset.accept(frame, self.part_limit),
            Some(Err(error)) => Err(error),
            None if matches!(self.reset, Reset::Collecting(_)) => Err(Status::data_loss(
                "connection snapshot ended before completion",
            )),
            None => {
                self.inner = None;
                return Poll::Ready(None);
            }
        };
        if result.is_err() {
            self.inner = None;
        }
        Poll::Ready(Some(result))
    }
}

/// A single subscription that delivers the initial snapshot as one complete reset.
#[derive(Debug)]
pub struct ConnectionEventStream {
    frames: ValidatedFrames,
    events: Vec<ConnectionEvent>,
}

impl ConnectionEventStream {
    pub(crate) fn new(inner: Streaming<ConnectionEvents>) -> Self {
        Self {
            frames: ValidatedFrames::new(inner, CONNECTION_EVENTS_MAX_FRAME_BYTES),
            events: Vec::new(),
        }
    }

    /// Read the next reset or delta. Dropping a pending read preserves staged data.
    pub async fn message(&mut self) -> Result<Option<ConnectionEvents>, Status> {
        self.next().await.transpose()
    }
}

impl Stream for ConnectionEventStream {
    type Item = Result<ConnectionEvents, Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        while let Some(frame) = ready!(self.frames.poll_next(cx)) {
            match frame {
                Ok(ConnectionEventFrame::Events(frame)) => return Poll::Ready(Some(Ok(frame))),
                Ok(ConnectionEventFrame::SnapshotPart(frame)) => self.events.extend(frame.events),
                Ok(ConnectionEventFrame::SnapshotComplete) => {
                    return Poll::Ready(Some(Ok(ConnectionEvents {
                        events: std::mem::take(&mut self.events),
                        reset: true,
                        snapshot_complete: None,
                    })));
                }
                Err(error) => {
                    self.events = Vec::new();
                    return Poll::Ready(Some(Err(error)));
                }
            }
        }
        Poll::Ready(None)
    }
}

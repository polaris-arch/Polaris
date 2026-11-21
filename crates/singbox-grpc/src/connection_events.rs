use crate::daemon::{ConnectionEvent, ConnectionEvents};
use crate::CONNECTION_EVENTS_MAX_FRAME_BYTES;
use futures_core::{ready, Stream};
use futures_util::StreamExt;
use prost::Message;
use std::pin::Pin;
use std::task::{Context, Poll};
use tonic::{Status, Streaming};

#[derive(Debug)]
enum Reset {
    Initial,
    Collecting {
        events: Vec<ConnectionEvent>,
        bytes: usize,
    },
    Complete,
}

/// A single subscription that delivers the initial snapshot as one complete reset.
#[derive(Debug)]
pub struct ConnectionEventStream {
    inner: Option<Streaming<ConnectionEvents>>,
    reset: Reset,
}

impl ConnectionEventStream {
    pub(crate) fn new(inner: Streaming<ConnectionEvents>) -> Self {
        Self {
            inner: Some(inner),
            reset: Reset::Initial,
        }
    }

    /// Read the next reset or delta. Dropping a pending read preserves staged data.
    pub async fn message(&mut self) -> Result<Option<ConnectionEvents>, Status> {
        self.next().await.transpose()
    }

    fn accept(&mut self, frame: ConnectionEvents) -> Result<Option<ConnectionEvents>, Status> {
        match (&mut self.reset, frame.snapshot_complete) {
            (Reset::Initial | Reset::Complete, None) => {
                // An older server ignores the opt-in and sends the whole reset in one frame.
                self.reset = Reset::Complete;
                Ok(Some(frame))
            }
            (Reset::Initial, Some(false)) if frame.reset => {
                self.reset = Reset::Collecting {
                    bytes: frame.encoded_len(),
                    events: frame.events,
                };
                Ok(None)
            }
            (Reset::Collecting { events, bytes }, Some(false))
                if !frame.reset && !frame.events.is_empty() =>
            {
                *bytes += frame.encoded_len();
                if *bytes > CONNECTION_EVENTS_MAX_FRAME_BYTES {
                    return Err(Status::resource_exhausted(
                        "connection snapshot exceeds its byte limit",
                    ));
                }
                events.extend(frame.events);
                Ok(None)
            }
            (Reset::Collecting { events, .. }, Some(true))
                if !frame.reset && frame.events.is_empty() =>
            {
                let reset = ConnectionEvents {
                    events: std::mem::take(events),
                    reset: true,
                    snapshot_complete: None,
                };
                self.reset = Reset::Complete;
                Ok(Some(reset))
            }
            _ => Err(Status::data_loss("invalid connection snapshot framing")),
        }
    }
}

impl Stream for ConnectionEventStream {
    type Item = Result<ConnectionEvents, Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        while let Some(stream) = &mut self.inner {
            let result = match ready!(stream.poll_next_unpin(cx)) {
                Some(Ok(frame)) => self.accept(frame),
                Some(Err(error)) => Err(error),
                None if matches!(self.reset, Reset::Collecting { .. }) => Err(Status::data_loss(
                    "connection snapshot ended before completion",
                )),
                None => {
                    self.inner = None;
                    break;
                }
            };
            match result {
                Ok(Some(frame)) => return Poll::Ready(Some(Ok(frame))),
                Ok(None) => {}
                Err(error) => {
                    self.inner = None;
                    self.reset = Reset::Complete;
                    return Poll::Ready(Some(Err(error)));
                }
            }
        }
        Poll::Ready(None)
    }
}

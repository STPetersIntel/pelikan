// Copyright 2021 Twitter, Inc.
// Licensed under the Apache License, Version 2.0
// http://www.apache.org/licenses/LICENSE-2.0

//! The server thread which accepts new connections, handles TLS handshaking,
//! and sends established sessions to the worker thread(s).

use super::EventLoop;
use crate::poll::{Poll, LISTENER_TOKEN, WAKER_TOKEN};
use crate::*;
use common::signal::Signal;
use common::ssl::{HandshakeError, MidHandshakeSslStream, Ssl, SslContext, SslStream};
// stefan: add WorkerConfig and TcpConfig
use config::{ServerConfig, WorkerConfig, TcpConfig};
use config::worker::Balance;
use mio::event::Event;
use mio::Events;
use mio::Token;
use queues::*;
use session::{Session, TcpStream};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

counter!(SERVER_EVENT_ERROR);
counter!(SERVER_EVENT_WRITE);
counter!(SERVER_EVENT_READ);
counter!(SERVER_EVENT_LOOP);
counter!(SERVER_EVENT_TOTAL);

pub struct ListenerBuilder {
    addr: SocketAddr,
    max_buffer_size: usize,
    nevent: usize,
    poll: Poll,
    ssl_context: Option<SslContext>,
    timeout: Duration,
    balance: Balance,
    priority: u32,
    workers: usize,
}

impl ListenerBuilder {
    /// Creates a new `Listener` from a `ServerConfig` and an optional
    /// `SslContext`.
    pub fn new<T: ServerConfig + WorkerConfig + TcpConfig>(
        config: &T,
        ssl_context: Option<SslContext>,
        max_buffer_size: usize,
    ) -> Result<Self, std::io::Error> {
        let workers = config.worker().threads();
        let balance = config.worker().balance();
        let priority = config.tcp().priority();
        let config = config.server();

        let addr = config.socket_addr().map_err(|e| {
            error!("{}", e);
            std::io::Error::new(std::io::ErrorKind::Other, "Bad listen address")
        })?;
        let mut poll = Poll::new().map_err(|e| {
            error!("{}", e);
            std::io::Error::new(std::io::ErrorKind::Other, "Failed to create epoll instance")
        })?;
        
        poll.bind(addr)?;

        let nevent = config.nevent();
        let timeout = Duration::from_millis(config.timeout() as u64);

        Ok(Self {
            addr,
            nevent,
            poll,
            ssl_context,
            timeout,
            max_buffer_size,
            balance,
            priority,
            workers
        })
    }

    pub fn waker(&self) -> Arc<Waker> {
        self.poll.waker()
    }

    pub fn build(
        self,
        signal_queue: Queues<(), Signal>,
        session_queue: Queues<Session, ()>,
    ) -> Listener {
        Listener {
            addr: self.addr,
            max_buffer_size: self.max_buffer_size,
            nevent: self.nevent,
            poll: self.poll,
            ssl_context: self.ssl_context,
            timeout: self.timeout,
            signal_queue,
            session_queue,
            balance: self.balance,
            priority: self.priority,
            workers: self.workers,
            napi_ids: Vec::new(),
        }
    }
}

pub struct Listener {
    addr: SocketAddr,
    max_buffer_size: usize,
    nevent: usize,
    poll: Poll,
    ssl_context: Option<SslContext>,
    timeout: Duration,
    signal_queue: Queues<(), Signal>,
    session_queue: Queues<Session, ()>,
    balance: Balance,
    priority: u32,
    workers: usize,
    napi_ids: Vec<u32>,
}

impl Listener {
    /// Call accept one time
    // TODO(bmartin): splitting accept and negotiation into separate threads
    // would allow us to handle TLS handshake with multiple threads and avoid
    // the overhead of re-registering the listener after each accept.
    fn do_accept(&mut self) {
        if let Ok((stream, _)) = self.poll.accept() {
            // stefan: set socket priority
            stream.set_priority(self.priority)
                .expect("Error setting socket priority");
            // handle TLS if it is configured
            if let Some(ssl_context) = &self.ssl_context {
                match Ssl::new(ssl_context).map(|v| v.accept(stream)) {
                    // handle case where we have a fully-negotiated
                    // TLS stream on accept()
                    Ok(Ok(tls_stream)) => {
                        self.add_established_tls_session(tls_stream);
                    }
                    // handle case where further negotiation is
                    // needed
                    Ok(Err(HandshakeError::WouldBlock(tls_stream))) => {
                        self.add_handshaking_tls_session(tls_stream);
                    }
                    // some other error has occurred and we drop the
                    // stream
                    Ok(Err(e)) => {
                        error!("accept failed: {}", e);
                        TCP_ACCEPT_EX.increment();
                    }
                    Err(e) => {
                        error!("accept failed: {}", e);
                        TCP_ACCEPT_EX.increment();
                    }
                }
            } else {
                self.add_plain_session(stream);
            };
            self.poll.reregister(LISTENER_TOKEN);
        }
    }

    /// Adds a new fully established TLS session
    fn add_established_tls_session(&mut self, stream: SslStream<TcpStream>) {
        match self.balance {
            Balance::Random => {
                let session =
                    Session::tls_with_capacity(stream, crate::DEFAULT_BUFFER_SIZE, self.max_buffer_size);
                trace!("accepted new session: {:?}", session);
                if self.session_queue.try_send_any(session).is_err() {
                    error!("error sending session to random worker");
                    TCP_ACCEPT_EX.increment();
                }
            }
            Balance::Queues => {
                let napi_id: u32 = stream.get_ref().napi_id().unwrap_or(0);
                let session =
                    Session::tls_with_capacity(stream, crate::DEFAULT_BUFFER_SIZE, self.max_buffer_size);
                trace!("accepted new session: {:?}", session);
                if napi_id > 0 {
                    let worker_id = self.napi_ids.iter().position(|&v| v == napi_id)
                        .unwrap_or_else(|| {
                            info!("found new napi_id {}", napi_id);
                            self.napi_ids.push(napi_id);
                            self.napi_ids.len() - 1
                        }) % self.workers;
                    trace!("napi_id {} mapped to worker {}", napi_id, worker_id);
                    if self.session_queue.try_send_to(worker_id, session).is_err() {
                        error!("error sending session to worker {}", worker_id);
                        TCP_ACCEPT_EX.increment();
                    }
                } else {
                    if self.session_queue.try_send_any(session).is_err() {
                        error!("error sending session to random worker");
                        TCP_ACCEPT_EX.increment();
                    }
                }
            }
        }
    }

    /// Adds a new TLS session that requires further handshaking
    fn add_handshaking_tls_session(&mut self, stream: MidHandshakeSslStream<TcpStream>) {
        let session = Session::handshaking_with_capacity(
            stream,
            crate::DEFAULT_BUFFER_SIZE,
            self.max_buffer_size,
        );
        if self.poll.add_session(session).is_err() {
            error!("failed to register handshaking TLS session with epoll");
            TCP_ACCEPT_EX.increment();
        }
    }

    /// Adds a new plain (non-TLS) session
    fn add_plain_session(&mut self, stream: TcpStream) {
        match self.balance {
            Balance::Random => {
                let session =
                    Session::plain_with_capacity(stream, crate::DEFAULT_BUFFER_SIZE, self.max_buffer_size);
                trace!("accepted new session: {:?}", session);
                if self.session_queue.try_send_any(session).is_err() {
                    error!("error sending session to random worker");
                    TCP_ACCEPT_EX.increment();
                }
            }
            Balance::Queues => {
                let napi_id: u32 = stream.napi_id().unwrap_or(0);
                let session =
                    Session::plain_with_capacity(stream, crate::DEFAULT_BUFFER_SIZE, self.max_buffer_size);
                trace!("accepted new session: {:?}", session);
                if napi_id > 0 {
                    let worker_id = self.napi_ids.iter().position(|&v| v == napi_id)
                        .unwrap_or_else(|| {
                            info!("found new napi_id {}", napi_id);
                            self.napi_ids.push(napi_id);
                            self.napi_ids.len() - 1
                        }) % self.workers;
                    trace!("napi_id {} mapped to worker {}", napi_id, worker_id);
                    if self.session_queue.try_send_to(worker_id, session).is_err() {
                        error!("error sending session to worker {}", worker_id);
                        TCP_ACCEPT_EX.increment();
                    }
                } else {
                    if self.session_queue.try_send_any(session).is_err() {
                        error!("error sending session to random worker");
                        TCP_ACCEPT_EX.increment();
                    }
                }
            }
        }
    }

    /// Handle an event on an existing session
    fn handle_session_event(&mut self, event: &Event) {
        let token = event.token();

        // handle error events first
        if event.is_error() {
            SERVER_EVENT_ERROR.increment();
            self.handle_error(token);
        }

        // handle write events before read events to reduce write
        // buffer growth if there is also a readable event
        if event.is_writable() {
            SERVER_EVENT_WRITE.increment();
            self.do_write(token);
        }

        // read events are handled last
        if event.is_readable() {
            SERVER_EVENT_READ.increment();
            let _ = self.do_read(token);
        }

        if let Ok(session) = self.poll.get_mut_session(token) {
            if session.do_handshake().is_ok() {
                trace!("handshake complete for session: {:?}", session);
                if let Ok(session) = self.poll.remove_session(token) {
                    if self.session_queue.try_send_any(session).is_err() {
                        error!("error sending session to worker");
                        TCP_ACCEPT_EX.increment();
                    }
                } else {
                    error!("error removing session from poller");
                    TCP_ACCEPT_EX.increment();
                }
            } else {
                trace!("handshake incomplete for session: {:?}", session);
            }
        }
    }

    /// Runs the `Listener` in a loop, accepting new sessions and moving them to
    /// a worker queue.
    pub fn run(&mut self) {
        info!("running server on: {}", self.addr);

        let mut events = Events::with_capacity(self.nevent);

        // repeatedly run accepting new connections and moving them to the worker
        loop {
            SERVER_EVENT_LOOP.increment();
            if self.poll.poll(&mut events, self.timeout).is_err() {
                error!("Error polling server");
            }
            SERVER_EVENT_TOTAL.add(events.iter().count() as _);

            // handle all events
            for event in events.iter() {
                match event.token() {
                    LISTENER_TOKEN => {
                        self.do_accept();
                    }
                    WAKER_TOKEN => {
                        while let Some(signal) =
                            self.signal_queue.try_recv().map(|v| v.into_inner())
                        {
                            match signal {
                                Signal::FlushAll => {}
                                Signal::Shutdown => {
                                    return;
                                }
                            }
                        }
                    }
                    _ => {
                        self.handle_session_event(event);
                    }
                }
            }

            let _ = self.session_queue.wake();
        }
    }
}

impl EventLoop for Listener {
    fn handle_data(&mut self, _token: Token) -> Result<(), std::io::Error> {
        Ok(())
    }

    fn poll(&mut self) -> &mut Poll {
        &mut self.poll
    }
}

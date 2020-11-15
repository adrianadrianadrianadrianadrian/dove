/*
 * Copyright 2019-2020, Ulf Lilleengen
 * License: Apache License 2.0 (see the file LICENSE or http://apache.org/licenses/LICENSE-2.0.html).
 */

//! The client module contains a simple client API for sending and receiving messages.
use crate::conn;
use crate::conn::{ChannelId, ConnectionOptions};
use crate::error::*;
use crate::framing::{AmqpFrame, Close, Frame, LinkRole, Open, Performative};
use log::trace;
// use mio::event;
use mio::{Events, Interest, Poll, Token};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
// use crate::message::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub type DeliveryTag = Vec<u8>;
type HandleId = u32;

pub struct Client {
    container_id: String,
    poll: Mutex<Poll>,
    incoming: Channel<Token>,
    connections: Mutex<HashMap<Token, Arc<ConnectionInner>>>,
}

pub struct Connection {
    connection: Arc<ConnectionInner>,

    pub container_id: String,
    pub hostname: String,
    pub channel_max: u16,
    pub idle_timeout: Duration,

    pub remote_idle_timeout: Duration,
    pub remote_container_id: String,
    pub remote_channel_max: u16,
}

struct ConnectionInner {
    channel_max: u16,
    idle_timeout: Duration,
    driver: Mutex<conn::Connection>,
    sessions: Mutex<HashMap<ChannelId, Arc<SessionInner>>>,

    // Frames received on this connection
    rx: Channel<Performative>,
    remote_channel_map: Mutex<HashMap<ChannelId, ChannelId>>,
    remote_idle_timeout: Duration,
}

pub struct Session {
    connection: Arc<ConnectionInner>,
    session: Arc<SessionInner>,
}

struct SessionInner {
    // Frames received on this session
    rx: Channel<Performative>,
    links: Mutex<HashMap<HandleId, Arc<LinkInner>>>,
    //   driver: Arc<Mutex<conn::Connection>>,
    /*    channel: ChannelId,
    driver: Arc<Mutex<conn::Connection>>,
    opened: Channel<()>,

    end_condition: Option<ErrorCondition>,
    remote_channel: Option<ChannelId>,
    handle_max: u32,
    delivery_to_handle: HashMap<u32, HandleId>,
    next_outgoing_id: u32,

    opts: SessionOpts,
    incoming: Channel<HandleId>,
    */
}

pub struct Link {
    connection: Arc<ConnectionInner>,
    link: Arc<LinkInner>,
    /*
    driver: Arc<Mutex<conn::Connection>>,
    channel: ChannelId,
    handle: HandleId,
    opened: Channel<()>,
    */
}

struct LinkInner {
    rx: Channel<Performative>,
    deliveries: Mutex<HashMap<DeliveryTag, Arc<Channel<Disposition>>>>,
}

pub struct SessionOpts {
    pub max_frame_size: u32,
}

impl Client {
    pub fn new() -> Result<Client> {
        let p = Poll::new()?;
        Ok(Client {
            container_id: "rs-amqp10".to_string(),
            incoming: Channel::new(),
            poll: Mutex::new(p),
            connections: Mutex::new(HashMap::new()),
        })
    }

    pub async fn connect(
        &self,
        host: &str,
        port: u16,
        opts: ConnectionOptions,
    ) -> Result<Connection> {
        let mut driver = conn::connect(host, port, opts)?;
        println!("Connected! Sending open...");

        let mut open = Open::new(self.container_id.as_str());
        open.hostname = Some(host.to_string());
        open.channel_max = Some(std::u16::MAX);
        open.idle_timeout = Some(5000);
        driver.open(open)?;

        let conn = Arc::new(ConnectionInner {
            driver: Mutex::new(driver),
            rx: Channel::new(),
            sessions: Mutex::new(HashMap::new()),
            remote_channel_map: Mutex::new(HashMap::new()),
            idle_timeout: Duration::from_secs(5),
            remote_idle_timeout: Duration::from_secs(0),
            channel_max: std::u16::MAX,
        });

        // TODO: Increment
        let id = Token(1);
        {
            let mut m = self.connections.lock().unwrap();
            m.insert(id, conn.clone());
        }
        self.incoming.send(id)?;

        println!("Waiting until opened...");
        loop {
            let frame = conn.recv().await?;
            match frame {
                Performative::Open(o) => {
                    // Populate remote properties
                    return Ok(Connection {
                        connection: conn.clone(),
                        container_id: self.container_id.clone(),
                        hostname: host.to_string(),
                        channel_max: std::u16::MAX,
                        idle_timeout: Duration::from_secs(5),

                        remote_container_id: o.container_id.clone(),
                        remote_channel_max: o.channel_max.unwrap_or(std::u16::MAX),
                        remote_idle_timeout: Duration::from_millis(
                            o.idle_timeout.unwrap_or(0) as u64
                        ),
                    });
                }
                _ => {
                    // Push it back into the queue
                    // TODO: Prevent reordering
                    conn.rx.send(frame)?;
                }
            }
        }
    }

    pub fn process(&self) -> Result<()> {
        let mut events = Events::with_capacity(1024);

        let mut tokens = HashSet::new();

        // Register new connections
        loop {
            let result = self.incoming.try_recv();
            match result {
                Err(_) => break,
                Ok(id) => {
                    let mut m = self.connections.lock().unwrap();
                    let conn = m.get_mut(&id).unwrap();
                    let mut driver = conn.driver.lock().unwrap();
                    self.poll.lock().unwrap().registry().register(
                        &mut *driver,
                        id,
                        Interest::READABLE | Interest::WRITABLE,
                    )?;
                    tokens.insert(id);
                }
            }
        }

        // Push connection frames on the wire
        for (id, connection) in self.connections.lock().unwrap().iter_mut() {
            let mut driver = connection.driver.lock().unwrap();

            // Handle keepalive
            connection.keepalive(&mut driver);

            // Flush data
            println!("Flushing driver");
            let result = driver.flush();
            match result {
                Err(_) => {
                    println!("Error flushing connection {:?}", id);
                }
                _ => {}
            }
        }

        // Poll for new events
        {
            self.poll
                .lock()
                .unwrap()
                .poll(&mut events, Some(Duration::from_millis(100)))?;
        }

        for event in &events {
            let id = event.token();
            println!("Got event for {:?}", id);
            let connection = {
                let m = self.connections.lock().unwrap();
                m.get(&id).map(|c| c.clone())
            };
            match connection {
                Some(c) => c.process()?,
                _ => {}
            }
        }
        Ok(())
    }
}

impl ConnectionInner {
    fn keepalive(&self, connection: &mut conn::Connection) -> Result<()> {
        // Sent out keepalives...
        let now = Instant::now();

        let last_received = connection.keepalive(self.remote_idle_timeout, now)?;
        if self.idle_timeout.as_millis() > 0 {
            // Ensure our peer honors our keepalive
            if now - last_received > self.idle_timeout * 2 {
                connection.close(Close {
                    error: Some(ErrorCondition {
                        condition: condition::RESOURCE_LIMIT_EXCEEDED.to_string(),
                        description: "local-idle-timeout expired".to_string(),
                    }),
                })?;
            }
        }
        Ok(())
    }

    fn process(&self) -> Result<()> {
        // Read frames until we're blocked
        let mut rx_frames = Vec::new();
        {
            let mut driver = self.driver.lock().unwrap();
            loop {
                let result = driver.process(&mut rx_frames);
                match result {
                    Ok(_) => {}
                    // This means that we should poll again to await further I/O action for this driver.
                    Err(AmqpError::IoError(ref e))
                        if e.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        break;
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
        }

        println!("Got {:?} frames", rx_frames.len());

        self.dispatch(rx_frames)
    }

    fn dispatch(&self, mut frames: Vec<Frame>) -> Result<()> {
        // Process received frames.
        for frame in frames.drain(..) {
            if let Frame::AMQP(AmqpFrame {
                channel,
                performative,
                payload,
            }) = frame
            {
                if channel == 0 {
                    println!("Got AMQP frame: {:?}", performative);
                    self.rx.send(performative.unwrap())?;
                } else {
                    match performative.unwrap() {
                        Performative::Begin(begin) => {
                            let mut m = self.sessions.lock().unwrap();
                            m.get_mut(&channel)
                                .map(|s| s.rx.send(Performative::Begin(begin)));
                        }
                        Performative::End(end) => {
                            let mut m = self.sessions.lock().unwrap();
                            m.get_mut(&channel)
                                .map(|s| s.rx.send(Performative::End(end)));
                        }
                        p => {
                            let session = {
                                let mut m = self.sessions.lock().unwrap();
                                m.get_mut(&channel).map(|s| s.clone())
                            };

                            match session {
                                Some(s) => {
                                    s.dispatch(p, payload)?;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn allocate_session(
        self: &Self,
        remote_channel_id: Option<ChannelId>,
    ) -> Option<Arc<SessionInner>> {
        let mut m = self.sessions.lock().unwrap();
        for i in 0..self.channel_max {
            let chan = i as ChannelId;
            if !m.contains_key(&chan) {
                let session = Arc::new(SessionInner {
                    rx: Channel::new(),
                    links: Mutex::new(HashMap::new()),
                    //             driver: self.inner.clone(),
                    /*
                    remote_channel: remote_channel_id,
                    local_channel: chan,
                    handle_max: std::u32::MAX,
                    delivery_to_handle: HashMap::new(),
                    next_outgoing_id: 0,

                    opts: None,
                    incoming: Channel::new(),
                    opened: Channel::new(),
                    */
                });
                m.insert(chan, session.clone());
                remote_channel_id.map(|c| self.remote_channel_map.lock().unwrap().insert(c, chan));
                return Some(session);
            }
        }
        None
    }

    pub async fn new_session(&self, _opts: Option<SessionOpts>) -> Result<Arc<SessionInner>> {
        Ok(self.allocate_session(None).unwrap())
    }

    pub async fn recv(&self) -> Result<Performative> {
        self.rx.recv()
    }
}

impl Connection {
    pub async fn new_session(&self, opts: Option<SessionOpts>) -> Result<Session> {
        let s = self.connection.new_session(opts).await?;

        println!("Waiting until begin...");
        loop {
            let frame = self.connection.recv().await?;
            match frame {
                Performative::Begin(b) => {
                    // Populate remote properties
                    return Ok(Session {
                        connection: self.connection.clone(),
                        session: s,
                    });
                }
                _ => {
                    // Push it back into the queue
                    // TODO: Prevent reordering
                    self.connection.rx.send(frame)?;
                }
            }
        }
    }
}

impl SessionInner {
    pub fn dispatch(&self, performative: Performative, payload: Option<Vec<u8>>) -> Result<()> {
        match performative {
            Performative::Attach(attach) => {}
            Performative::Detach(detach) => {}
            Performative::Transfer(transfer) => {}
            Performative::Disposition(disposition) => {}
            Performative::Flow(flow) => {}
            _ => {
                println!("Unexpected performative for session: {:?}", performative);
            }
        }
        Ok(())
    }

    pub fn new_link(&self, addr: &str, role: LinkRole) -> Result<Arc<LinkInner>> {
        let link = Arc::new(LinkInner {
            rx: Channel::new(),
            deliveries: Mutex::new(HashMap::new()),
        });
        // TODO: Increment id
        self.links.lock().unwrap().insert(0, link.clone());
        Ok(link)
    }
}

impl Session {
    pub async fn new_sender(&self, addr: &str) -> Result<Link> {
        self.new_link(addr, LinkRole::Sender).await
    }

    async fn new_link(&self, addr: &str, role: LinkRole) -> Result<Link> {
        let link = self.session.new_link(addr, role)?;
        Ok(Link {
            connection: self.connection.clone(),
            link: link,
        })
        /*
        let address = Some(addr.to_string());
            let link = Link {
                name: addr.to_string(),
                handle: 1,
                next_message_id: 0,
                role: LinkRole::Sender,
                source: Some(Source {
                    address: None,
                    durable: None,
                    expiry_policy: None,
                    timeout: None,
                    dynamic: None,
                    dynamic_node_properties: None,
                    default_outcome: None,
                    distribution_mode: None,
                    filter: None,
                    outcomes: None,
                    capabilities: None,
                }),
                target: Some(Target {
                    address: address.map(|s| s.to_string()),
                    durable: None,
                    expiry_policy: None,
                    timeout: None,
                    dynamic: Some(false),
                    dynamic_node_properties: None,
                    capabilities: None,
                }),
                deliveries: Vec::new(),
                dispositions: Vec::new(),
                unsettled: HashMap::new(),
                flow: Vec::new(),
            };
        let id = {
            let mut driver = self.driver.lock().unwrap();

        );

        self.incoming.send(id)?;

        // Wait until it has been opened
        let s = {
            let mut m = self.links.lock().unwrap();
            m.get_mut(&id).unwrap().clone()
        };
        println!("Awaiting link to be opened");
        let _ = s.opened.recv()?;
        println!("LINK OPENED");
        Ok(s)
        */
    }

    pub async fn new_receiver(&self, addr: &str) -> Result<Link> {
        self.new_link(addr, LinkRole::Receiver).await
    }
}

impl Link {
    pub async fn send(&self, _data: &str) -> Result<Disposition> {
        Ok(Disposition {})
        /*
        let mut driver = self.driver.lock().unwrap();
        let session = driver.get_session(self.channel).unwrap();
        let link = session.get_link(self.handle).unwrap();
        let c = Arc::new(Channel::new());
        {
            let mut m = self.deliveries.lock().unwrap();
            let delivery = link.send(data)?;
            m.insert(delivery.tag, c.clone());
        }

        let disposition = c.recv()?;

        return Ok(disposition);
        */
    }

    pub async fn receive(&self) -> Result<Delivery> {
        return Ok(Delivery {});
    }
}

pub struct Disposition {
    /*    pub remote_settled: bool,
pub state: Option<DeliveryState>,*/}

pub struct Delivery {}

impl Delivery {
    pub fn message(&self) -> Result<String> {
        return Ok(String::new());
    }
}

pub struct Channel<T> {
    tx: Mutex<mpsc::Sender<T>>,
    rx: Mutex<mpsc::Receiver<T>>,
}

impl<T> Channel<T> {
    fn new() -> Channel<T> {
        let (tx, rx) = mpsc::channel();
        return Channel {
            tx: Mutex::new(tx),
            rx: Mutex::new(rx),
        };
    }

    fn send(&self, value: T) -> Result<()> {
        self.tx.lock().unwrap().send(value)?;
        Ok(())
    }

    fn try_recv(&self) -> Result<T> {
        let r = self.rx.lock().unwrap().try_recv()?;
        Ok(r)
    }

    fn recv(&self) -> Result<T> {
        let r = self.rx.lock().unwrap().recv()?;
        Ok(r)
    }
}

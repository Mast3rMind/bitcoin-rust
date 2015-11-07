extern crate mio;

use std::net::SocketAddr;
use mio::tcp;

use super::rpcengine::RPCEngine;
use super::rpcengine;
use super::messages::*;

use super::Services;
use super::IPAddress;

use time;

use std::net::Ipv6Addr;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, Arc};
use std::thread;
use std::mem;

use mio::Token;

struct BitcoinClient {
    version: i32,
    services: Services,
    user_agent: String,
    state: Arc<Mutex<State>>,
}

struct State {
    peers: HashMap<mio::Token, Peer>,
    message_queue: VecDeque<Vec<u8>>,
    network_type: NetworkType,
}

impl State {
    pub fn new(network_type: NetworkType) -> State {
        State {
            peers: HashMap::new(),
            message_queue: VecDeque::new(),
            network_type: network_type,
        }
    }

    pub fn add_peer(&mut self, token: mio::Token, version: VersionMessage) {
        self.peers.insert(token, Peer::new(time::now(), version));
        println!("peers = {:?}", self.peers);
    }

    pub fn queue_message(&mut self, command: Command, message: Option<Box<Serializable>>) {
        let to_send = get_serialized_message(self.network_type,
                                             command,
                                             message);

        self.message_queue.push_back(to_send);
    }

    pub fn get_peers(&self) -> &HashMap<mio::Token, Peer> { &self.peers }
}

#[derive(Debug)]
struct Peer {
    last_ping: time::Tm,
    version: VersionMessage,
}

impl Peer {
    pub fn new(last_ping: time::Tm, version: VersionMessage) -> Peer {
        Peer {
            last_ping: last_ping,
            version: version,
        }
    }

    pub fn version(&self) -> &VersionMessage { &self.version }
}

impl BitcoinClient {
    fn new(state: Arc<Mutex<State>>) -> BitcoinClient {
        BitcoinClient {
            version: 70001,
            services: Services::new(true),
            user_agent: "/Agi:0.0.1/".to_string(),
            state: state,
        }
    }

    fn handle_verack(&self) {
        // TODO: register verack
    }

    fn generate_version_message(&self, recipient_ip: IPAddress) -> VersionMessage {
        VersionMessage {
            version: self.version,
            services: self.services,
            timestamp: time::now(),
            addr_recv: IPAddress::new(
                time::now(),
                self.services,
                // TODO: use upnp
                "0:0:0:0:0:ffff:c0a8:3865".parse().unwrap(),
                18334),
            addr_from: recipient_ip,
            // TODO: figure it out this
            nonce: 1234,
            user_agent: self.user_agent.clone(),
            start_height: 0,
            relay: true,
        }
    }

    fn handle_version(&mut self, token: mio::Token, message: VersionMessage) {
        let mut state = self.state.lock().unwrap();

        let version = self.generate_version_message(message.addr_recv);
        state.add_peer(token, message);

        state.queue_message(Command::Version, Some(Box::new(version)));
        state.queue_message(Command::Verack, None);
    }

    fn handle_addr(&mut self, _: AddrMessage) {
        unimplemented!()
    }

    fn handle_getaddr(&self) {
        let mut state = self.state.lock().unwrap();

        let mut peers = vec![];
        for peer in state.get_peers().values() {
            peers.push((peer.last_ping, peer.version.addr_from));
        }

        let response = AddrMessage::new(peers);

        state.queue_message(Command::Addr, Some(Box::new(response)));
    }

    fn handle_pong(&self, _: PingMessage) {
        unimplemented!()
    }

    fn handle_reject(&self, message: RejectMessage) {
        println!("Message = {:?}", message);
    }

    fn handle_ping(&self, message: PingMessage) {
        self.lock_state()
            .queue_message(Command::Pong, Some(Box::new(message)));
    }

    fn lock_state<'a>(&'a self) -> MutexGuard<'a, State> { self.state.lock().unwrap() }

    fn handle_command(&mut self, header: MessageHeader, token: mio::Token,
                      message_bytes_: Vec<u8>) {
        if *header.magic() != self.lock_state().network_type {
            // This packet is not for the right version :O
            print!("Received packet for wrong version.");
            return;
        }

        let mut message_bytes = message_bytes_;
        let response = match header.command() {
            &Command::Pong => {
                println!("==== Got message Pong");
                // Ping and Pong message use the same format
                let message = PingMessage::deserialize(&mut message_bytes);
                self.handle_pong(message);
            },
            &Command::Ping => {
                println!("==== Got message Ping");
                let message = PingMessage::deserialize(&mut message_bytes);
                self.handle_ping(message);
            },
            &Command::Version => {
                println!("==== Got message Version");
                let message = VersionMessage::deserialize(&mut message_bytes);
                self.handle_version(token, message);
            },
            &Command::Verack => {
                println!("==== Got message Verack");
                self.handle_verack();
            },
            &Command::GetAddr => {
                println!("==== Got message GetAddr");
                self.handle_getaddr();
            },
            &Command::Addr => {
                println!("==== Got message GetAddr");
                let message = AddrMessage::deserialize(&mut message_bytes);
                self.handle_addr(message);
            },
            &Command::Reject => {
                println!("==== Got message Reject :-(");
                let message = RejectMessage::deserialize(&mut message_bytes);
                println!("message = {:?}", message_bytes);
                self.handle_reject(message);
            }
        };

        response
    }
}

impl rpcengine::MessageHandler for BitcoinClient {
    fn handle(&mut self, token: mio::Token, message_: Vec<u8>) -> VecDeque<Vec<u8>> {
        println!("Got RPC from {:?}", token);
        let mut message = message_;
        message.reverse();

        match MessageHeader::deserialize(&mut message) {
            Ok(x) => self.handle_command(x, token, message),
            Err(x) => println!("Error: {}", x),
        }

        let mut state = self.state.lock().unwrap();
        let message_queue = mem::replace(&mut state.message_queue, VecDeque::new());

        message_queue
    }

    fn get_new_messages(&mut self) -> HashMap<mio::Token, Vec<Vec<u8>>> {
        HashMap::new()
    }
}

pub fn start(address: SocketAddr) {
    let server = tcp::TcpListener::bind(&address).unwrap();
    let mut event_loop = mio::EventLoop::new().unwrap();
    event_loop.register(&server, rpcengine::SERVER).unwrap();
    let state = Arc::new(Mutex::new(State::new(NetworkType::TestNet3)));
    let client = BitcoinClient::new(state.clone());

    println!("running bitcoin server; port=18333");
    let child = thread::spawn(move || {
        let mut engine = RPCEngine::new(server, Box::new(client));
        event_loop.run(&mut engine).unwrap();
    });

    let _ = child.join();
}

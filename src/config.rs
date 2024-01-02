use std::str::FromStr;
use std::{sync::Arc, time::Duration};
use std::net::{UdpSocket, SocketAddr};

use libp2p_identity::{Keypair, PeerId};
use multiaddr::{Multiaddr, Protocol};

use napi::bindgen_prelude::*;
use napi_derive::napi;

use futures::{
    future::{select, Either, FutureExt, Select},
    prelude::*,
};

#[derive(Clone)]
#[napi(object)]
pub struct TransportConfig {
    pub keypair: Buffer,
    /// Timeout for the initial handshake when establishing a connection.
    /// The actual timeout is the minimum of this and the [`Config::max_idle_timeout`].
    pub handshake_timeout: u32,
    /// Maximum duration of inactivity in ms to accept before timing out the connection.
    pub max_idle_timeout: u32,
    /// Period of inactivity before sending a keep-alive packet.
    /// Must be set lower than the idle_timeout of both
    /// peers to be effective.
    ///
    /// See [`quinn::TransportConfig::keep_alive_interval`] for more
    /// info.
    pub keep_alive_interval: u32,
    /// Maximum number of incoming bidirectional streams that may be open
    /// concurrently by the remote peer.
    pub max_concurrent_stream_limit: u32,

    /// Max unacknowledged data in bytes that may be send on a single stream.
    pub max_stream_data: u32,

    /// Max unacknowledged data in bytes that may be send in total on all streams
    /// of a connection.
    pub max_connection_data: u32,
}


// handle defaults on JS level

// handshake_timeout: 5000,
// max_idle_timeout: 30 * 1000,
// max_concurrent_stream_limit: 256,
// keep_alive_interval: 15000,
// max_connection_data: 15_000_000,
// // Ensure that one stream is not consuming the whole connection.
// max_stream_data: 10_000_000,

/// Represents the inner configuration for [`quinn`].
#[derive(Debug, Clone)]
pub(crate) struct QuinnConfig {
    pub(crate) client_config: quinn::ClientConfig,
    pub(crate) server_config: quinn::ServerConfig,
    pub(crate) endpoint_config: quinn::EndpointConfig,
}

impl From<TransportConfig> for QuinnConfig {
    fn from(config: TransportConfig) -> QuinnConfig {
        let TransportConfig {
            keypair,
            max_idle_timeout,
            max_concurrent_stream_limit,
            keep_alive_interval,
            max_connection_data,
            max_stream_data,
            handshake_timeout: _,
        } = config;
        let mut transport = quinn::TransportConfig::default();
        // Disable uni-directional streams.
        transport.max_concurrent_uni_streams(0u32.into());
        transport.max_concurrent_bidi_streams(max_concurrent_stream_limit.into());
        // Disable datagrams.
        transport.datagram_receive_buffer_size(None);
        transport.keep_alive_interval(Some(Duration::from_millis(keep_alive_interval.into())));
        transport.max_idle_timeout(Some(quinn::VarInt::from_u32(max_idle_timeout).into()));
        transport.allow_spin(false);
        transport.stream_receive_window(max_stream_data.into());
        transport.receive_window(max_connection_data.into());
        transport.mtu_discovery_config(Some(Default::default()));
        let transport = Arc::new(transport);

        let keypair = Keypair::from_protobuf_encoding(&keypair).expect("Invalid keypair");

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(libp2p_tls::make_server_config(&keypair).unwrap()));
        server_config.transport = Arc::clone(&transport);
        // Disables connection migration.
        // Long-term this should be enabled, however we then need to handle address change
        // on connections in the `Connection`.
        server_config.migration(false);

        let mut client_config = quinn::ClientConfig::new(Arc::new(libp2p_tls::make_client_config(&keypair, None).unwrap()));
        client_config.transport_config(transport);

        let mut endpoint_config = keypair
            .derive_secret(b"libp2p quic stateless reset key")
            .map(|secret| {
                let reset_key = Arc::new(ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &secret));
                quinn::EndpointConfig::new(reset_key)
            })
            .unwrap_or_default();

        endpoint_config.supported_versions(vec![1]);

        QuinnConfig {
            client_config,
            server_config,
            endpoint_config,
        }
    }
}


/// Listener for incoming connections.
// struct Listener {
//     /// Id of the listener.
//     listener_id: usize,

//     /// Endpoint
//     endpoint: quinn::Endpoint,

//     /// An underlying copy of the socket to be able to hole punch with
//     socket: UdpSocket,

//     /// A future to poll new incoming connections.
//     accept: BoxFuture<'static, Option<quinn::Connecting>>,

//     /// Timeout for connection establishment on inbound connections.
//     handshake_timeout: Duration,

//     /// Whether the listener was closed and the stream should terminate.
//     is_closed: bool,

//     /// Pending event to reported.
//     pending_event: Option<<Self as Stream>::Item>,

//     /// The stream must be awaken after it has been closed to deliver the last event.
//     close_listener_waker: Option<Waker>,

//     listening_addresses: HashSet<IpAddr>,
// }

pub struct ListenerId(usize);

pub enum ListenerEvent {
    /// A new address is being listened on.
    Listening {
        /// The listener that is listening on the new address.
        listener_id: ListenerId,
        /// The new address that is being listened on.
        listen_addr: String,
    },
    /// An inbound Connection has been established
    IncomingConnection {
        listener_id: ListenerId,
        connection: Connection
    },
    /// An error has occured
    Error {
        listener_id: ListenerId,
        error: napi::Error
    },
    /// The listener has been closed
    Close {
        listener_id: ListenerId
    },
}

pub struct ConnectionId(usize);

pub enum ConnectionEvent {
    IncomingStream {
        connection_id: ConnectionId,
        stream: Stream
    },
    Error {
        connection_id: ConnectionId,
        error: napi::Error
    },
    Close {
        connection_id: ConnectionId
    }
}

#[napi]
pub struct Transport {
    quinn_config: QuinnConfig,
    /// quinn endpoint used for dialing
    dialer: quinn::Endpoint
}

#[napi]
pub struct Listener {
    endpoint: quinn::Endpoint
}

#[napi]
pub struct Connection {
    connection: quinn::Connection
}

#[napi]
pub struct Stream {
    send: quinn::SendStream,
    recv: quinn::RecvStream
}

#[napi]
impl Transport {

    #[napi(factory)]
    pub fn new(config: TransportConfig) -> Self {
        let quinn_config = QuinnConfig::from(config);
        // TODO add an IPv6 dialer?
        let dialer = quinn::Endpoint::new(
            quinn_config.endpoint_config.clone(),
            None,
            UdpSocket::bind("0.0.0.0:0").unwrap(),
            Arc::new(quinn::TokioRuntime)
        ).expect("Invalid dialer");
        Transport { quinn_config, dialer }
    }

    #[napi]
    pub async fn dial(&self, addr: String) -> Result<Connection> {
        let (socket_addr, _) = multiaddr_to_socketaddr(&Multiaddr::from_str(&addr).expect("foo")).expect("foo");
        let connection = self.dialer.connect_with(
            self.quinn_config.client_config.clone(),
            socket_addr,
            "l"
        ).expect("foo").await.expect("foo");
        Ok(Connection { connection })
    }
    #[napi]
    pub fn create_listener(&self, addr: String) -> Result<Listener> {
        let socket_addr = SocketAddr::from_str(&addr).expect("Invalid socket address");
        // let mu = Multiaddr::from_str(&addr).expect("foo");
        // println!("{}", mu);
        // let (socket_addr, _) = multiaddr_to_socketaddr(&mu).expect("foo");
        // println!("{}", socket_addr);
        let socket = UdpSocket::bind(socket_addr).expect("foo");
        match quinn::Endpoint::new(
            self.quinn_config.endpoint_config.clone(),
            Some(self.quinn_config.server_config.clone()),
            socket,
            Arc::new(quinn::TokioRuntime)
        ) {
            Ok(endpoint) => Ok(Listener { endpoint }),
            Err(error) => Err(error.into()),
        }
    }
}



/// Tries to turn a QUIC multiaddress into a UDP [`SocketAddr`]. Returns None if the format
/// of the multiaddr is wrong.
fn multiaddr_to_socketaddr(
    addr: &Multiaddr,
) -> Option<(SocketAddr, Option<PeerId>)> {
    let mut iter = addr.iter();
    let proto1 = iter.next()?;
    let proto2 = iter.next()?;
    let proto3 = iter.next()?;

    if proto3 != Protocol::QuicV1 {
        return None
    }

    let mut peer_id = None;
    for proto in iter {
        match proto {
            Protocol::P2p(id) => {
                peer_id = Some(id);
            }
            _ => return None,
        }
    }

    match (proto1, proto2) {
        (Protocol::Ip4(ip), Protocol::Udp(port)) => {
            Some((SocketAddr::new(ip.into(), port), peer_id))
        }
        (Protocol::Ip6(ip), Protocol::Udp(port)) => {
            Some((SocketAddr::new(ip.into(), port), peer_id))
        }
        _ => None,
    }
}

/// Whether an [`Multiaddr`] is a valid for the QUIC transport.
fn is_quic_addr(addr: &Multiaddr) -> bool {
    use Protocol::*;
    let mut iter = addr.iter();
    let Some(first) = iter.next() else {
        return false;
    };
    let Some(second) = iter.next() else {
        return false;
    };
    let Some(third) = iter.next() else {
        return false;
    };
    let fourth = iter.next();
    let fifth = iter.next();

    matches!(first, Ip4(_) | Ip6(_) | Dns(_) | Dns4(_) | Dns6(_))
        && matches!(second, Udp(_))
        && matches!(third, QuicV1)
        && matches!(fourth, Some(P2p(_)) | None)
        && fifth.is_none()
}

/// Turns an IP address and port into the corresponding QUIC multiaddr.
fn socketaddr_to_multiaddr(socket_addr: &SocketAddr) -> Multiaddr {
    let quic_proto = Protocol::QuicV1;

    Multiaddr::empty()
        .with(socket_addr.ip().into())
        .with(Protocol::Udp(socket_addr.port()))
        .with(quic_proto)
}
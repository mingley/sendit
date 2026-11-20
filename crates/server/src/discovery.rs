//! Opt-in directory registration and LAN discovery on the host's existing game socket.
use sendit_sim::{
    map::valid_map_name,
    wire::{decode, encode, PROTOCOL},
    Mode,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    ops::Bound::{Excluded, Unbounded},
    time::{Duration, Instant},
};

const MAGIC: &[u8; 5] = b"SDIR\x01";
const MAX_PACKET: usize = 384;
const QUERY_SIZE: usize = 256;
const HEARTBEAT: Duration = Duration::from_secs(15);
const CHALLENGE_TTL: Duration = Duration::from_secs(5);
const LISTING_TTL: Duration = Duration::from_secs(60);
const MAX_HOSTS: usize = 512;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Listing {
    pub name: String,
    pub map: String,
    pub mode: Mode,
    pub players: u16,
    pub cap: u16,
    pub locked: bool,
}

impl Listing {
    fn valid(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 96
            && !self.name.chars().any(char::is_control)
            && valid_map_name(&self.map)
            && (1..=32).contains(&self.cap)
            && self.players <= self.cap
    }
}

#[derive(Serialize, Deserialize)]
enum Message {
    Probe(u64),
    LanHost(u64, Listing),
    Register(u16),
    Challenge(u64),
    Claim(u64, Listing),
    Browse {
        nonce: u64,
        after: Option<SocketAddr>,
    },
    Page {
        nonce: u64,
        host: Option<(SocketAddr, Listing)>,
    },
}

pub fn matches(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

// Padding lets a browser budget enough bytes for a reply without turning either
// discovery service into an amplifier. The serialized payload still has a strict end.
fn packet(message: &Message, min_size: usize) -> Option<Vec<u8>> {
    let body = encode(message).ok()?;
    let size = (MAGIC.len() + 2 + body.len()).max(min_size);
    if size > MAX_PACKET {
        return None;
    }
    let mut data = Vec::with_capacity(size);
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&(body.len() as u16).to_le_bytes());
    data.extend_from_slice(&body);
    data.resize(size, 0);
    Some(data)
}

fn unpack(data: &[u8]) -> Option<Message> {
    if data.len() > MAX_PACKET {
        return None;
    }
    let rest = data.strip_prefix(MAGIC)?;
    let size = u16::from_le_bytes(rest.get(..2)?.try_into().ok()?) as usize;
    let (body, padding) = rest.get(2..)?.split_at_checked(size)?;
    if padding.iter().any(|&v| v != 0) {
        return None;
    }
    decode(body).ok()
}

fn send(socket: &UdpSocket, to: SocketAddr, message: &Message, min_size: usize) -> io::Result<()> {
    let bytes =
        packet(message, min_size).ok_or_else(|| io::Error::other("invalid discovery packet"))?;
    socket.send_to(&bytes, to)?;
    Ok(())
}

fn reply(socket: &UdpSocket, to: SocketAddr, message: &Message, request_size: usize) {
    if let Some(bytes) = packet(message, 0) {
        if bytes.len() <= request_size {
            let _ = socket.send_to(&bytes, to);
        }
    }
}

fn random_nonce() -> io::Result<u64> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(u64::from_ne_bytes(bytes))
}

fn usable_endpoint(address: SocketAddr) -> bool {
    address.port() != 0
        && !address.ip().is_multicast()
        && !address.ip().is_unspecified()
        && address.ip() != IpAddr::V4(Ipv4Addr::BROADCAST)
}

/// A disabled publisher can still answer LAN queries. Its game socket never contacts a directory.
pub struct Publisher {
    directory: Option<SocketAddr>,
    last_registration: Option<Instant>,
    awaiting_challenge: bool,
}

impl Publisher {
    pub fn new(directory: Option<SocketAddr>) -> Self {
        Self {
            directory,
            last_registration: None,
            awaiting_challenge: false,
        }
    }

    pub fn heartbeat(&mut self, socket: &UdpSocket, now: Instant, _info: &Listing) {
        let Some(directory) = self.directory else {
            return;
        };
        if self
            .last_registration
            .is_some_and(|last| now.saturating_duration_since(last) < HEARTBEAT)
        {
            return;
        }
        self.last_registration = Some(now);
        self.awaiting_challenge = send(socket, directory, &Message::Register(PROTOCOL), 32).is_ok();
    }

    pub fn receive(&mut self, socket: &UdpSocket, source: SocketAddr, data: &[u8], info: &Listing) {
        if !usable_endpoint(source) {
            return;
        }
        match unpack(data) {
            Some(Message::Probe(nonce)) => {
                reply(
                    socket,
                    source,
                    &Message::LanHost(nonce, info.clone()),
                    data.len(),
                );
            }
            Some(Message::Challenge(cookie))
                if Some(source) == self.directory && self.awaiting_challenge =>
            {
                self.awaiting_challenge = false;
                let _ = send(socket, source, &Message::Claim(cookie, info.clone()), 0);
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct Directory {
    // The only address ever listed is the verified UDP source address of the game socket.
    registered: BTreeMap<SocketAddr, (Listing, Instant)>,
    pending: BTreeMap<SocketAddr, (u64, Instant)>,
}

impl Directory {
    fn expire(&mut self, now: Instant) {
        self.registered
            .retain(|_, (_, seen)| now.saturating_duration_since(*seen) < LISTING_TTL);
        self.pending
            .retain(|_, (_, sent)| now.saturating_duration_since(*sent) < CHALLENGE_TTL);
    }

    fn receive(&mut self, socket: &UdpSocket, source: SocketAddr, data: &[u8], now: Instant) {
        if !usable_endpoint(source) {
            return;
        }
        match unpack(data) {
            Some(Message::Register(PROTOCOL)) => {
                if !self.registered.contains_key(&source) && self.registered.len() >= MAX_HOSTS {
                    return;
                }
                let cookie = match self.pending.get(&source) {
                    Some(&(cookie, at)) if now.saturating_duration_since(at) < CHALLENGE_TTL => {
                        cookie
                    }
                    _ if self.pending.len() < MAX_HOSTS => {
                        let Ok(cookie) = random_nonce() else { return };
                        self.pending.insert(source, (cookie, now));
                        cookie
                    }
                    _ => return,
                };
                reply(socket, source, &Message::Challenge(cookie), data.len());
            }
            Some(Message::Claim(cookie, info)) => {
                let valid = self.pending.get(&source).is_some_and(|&(expected, at)| {
                    expected == cookie && now.saturating_duration_since(at) < CHALLENGE_TTL
                });
                if valid
                    && info.valid()
                    && (self.registered.contains_key(&source) || self.registered.len() < MAX_HOSTS)
                {
                    self.pending.remove(&source);
                    self.registered.insert(source, (info, now));
                }
            }
            Some(Message::Browse { nonce, after }) => {
                let row = match after {
                    Some(cursor) => self.registered.range((Excluded(cursor), Unbounded)).next(),
                    None => self.registered.iter().next(),
                };
                let host = row.map(|(&addr, (info, _))| (addr, info.clone()));
                reply(socket, source, &Message::Page { nonce, host }, data.len());
            }
            _ => {}
        }
    }
}

fn directory(bind: SocketAddr) -> io::Result<()> {
    let socket = UdpSocket::bind(bind)?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    println!("SENDIT_DIRECTORY_READY addr={}", socket.local_addr()?);
    io::stdout().flush()?;
    let mut directory = Directory::default();
    let mut gc = Instant::now();
    let mut buffer = [0u8; MAX_PACKET + 1];
    while !crate::dedicated::stopping() {
        let now = Instant::now();
        if now >= gc {
            directory.expire(now);
            gc = now + Duration::from_secs(1);
        }
        match socket.recv_from(&mut buffer) {
            Ok((len, source)) => directory.receive(&socket, source, &buffer[..len], Instant::now()),
            Err(e) if transient(&e) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

fn source_socket(target: SocketAddr) -> io::Result<UdpSocket> {
    let bind = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind)?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    Ok(socket)
}

fn discover(target: SocketAddr) -> io::Result<BTreeMap<SocketAddr, Listing>> {
    let socket = source_socket(target)?;
    socket.set_broadcast(true)?;
    let nonce = random_nonce()?;
    let until = Instant::now() + Duration::from_millis(850);
    let mut retry = Instant::now();
    let mut result = BTreeMap::new();
    let mut buffer = [0u8; MAX_PACKET + 1];
    while Instant::now() < until {
        if Instant::now() >= retry {
            send(&socket, target, &Message::Probe(nonce), QUERY_SIZE)?;
            retry = Instant::now() + Duration::from_millis(300);
        }
        match socket.recv_from(&mut buffer) {
            Ok((len, source)) => {
                if let Some(Message::LanHost(reply, info)) = unpack(&buffer[..len]) {
                    if reply == nonce
                        && info.valid()
                        && usable_endpoint(source)
                        && result.len() < MAX_HOSTS
                    {
                        result.insert(source, info);
                    }
                }
            }
            Err(e) if transient(&e) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(result)
}

fn browse(target: SocketAddr) -> io::Result<BTreeMap<SocketAddr, Listing>> {
    let socket = source_socket(target)?;
    let mut hosts = BTreeMap::new();
    let mut after = None;
    let mut buffer = [0u8; MAX_PACKET + 1];
    // One bounded reply per request, with a stable address cursor.
    while hosts.len() < MAX_HOSTS {
        let nonce = random_nonce()?;
        let until = Instant::now() + Duration::from_millis(850);
        let mut retry = Instant::now();
        loop {
            if Instant::now() >= until {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "directory did not reply",
                ));
            }
            if Instant::now() >= retry {
                send(
                    &socket,
                    target,
                    &Message::Browse { nonce, after },
                    QUERY_SIZE,
                )?;
                retry = Instant::now() + Duration::from_millis(300);
            }
            match socket.recv_from(&mut buffer) {
                Ok((len, source)) if source == target => {
                    let Some(Message::Page {
                        nonce: echoed,
                        host,
                    }) = unpack(&buffer[..len])
                    else {
                        continue;
                    };
                    if echoed != nonce {
                        continue;
                    }
                    let Some((address, info)) = host else {
                        return Ok(hosts);
                    };
                    if !usable_endpoint(address)
                        || !info.valid()
                        || after.is_some_and(|prev| prev >= address)
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid directory entry",
                        ));
                    }
                    hosts.insert(address, info);
                    after = Some(address);
                    break;
                }
                Ok(_) => {}
                Err(e) if transient(&e) => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(hosts)
}

fn print_hosts(hosts: BTreeMap<SocketAddr, Listing>) {
    println!("hosts={}", hosts.len());
    for (addr, host) in hosts {
        let mode = match host.mode {
            Mode::Practice => "practice",
            Mode::Deathmatch => "deathmatch",
            Mode::Competitive => "competitive",
            Mode::Surf => "surf",
        };
        println!(
            "addr={addr} name={:?} map={} mode={mode} players={}/{} locked={}",
            host.name, host.map, host.players, host.cap, host.locked
        );
    }
}

pub fn command(args: &[String]) -> Option<Result<(), String>> {
    let first = args.first()?.as_str();
    if !matches!(first, "directory" | "discover" | "list") {
        return None;
    }
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let (action, target) = match (first, rest.as_slice()) {
        ("directory", []) => ("directory", "0.0.0.0:27016"),
        ("directory", ["--bind", addr]) => ("directory", *addr),
        ("discover", []) => ("discover", "255.255.255.255:27015"),
        ("discover", ["--target", addr]) => ("discover", *addr),
        ("list" | "discover", ["--directory", addr]) => ("list", *addr),
        _ => return Some(Err("use directory [--bind IP:PORT], discover [--target BROADCAST:PORT], or list --directory IP:PORT".into())),
    };
    let addr: SocketAddr = match target.parse() {
        Ok(addr) => addr,
        Err(_) => return Some(Err(format!("invalid IP:PORT for {first}: {target}"))),
    };
    let result = match action {
        "directory" => directory(addr),
        "discover" => discover(addr).map(print_hosts),
        _ => browse(addr).map(print_hosts),
    };
    Some(result.map_err(|e| e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Listing {
        Listing {
            name: "Local Club".into(),
            map: "relay".into(),
            mode: Mode::Competitive,
            players: 2,
            cap: 10,
            locked: false,
        }
    }
    fn udp() -> UdpSocket {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(250)))
            .unwrap();
        socket
    }
    #[test]
    fn registration_proves_return_address_and_has_a_finite_lease() {
        let directory_socket = udp();
        let host = udp();
        let from = host.local_addr().unwrap();
        let now = Instant::now();
        let mut dir = Directory::default();
        dir.receive(
            &directory_socket,
            from,
            &packet(&Message::Register(PROTOCOL), 32).unwrap(),
            now,
        );
        assert!(dir.registered.is_empty());
        let mut buf = [0; MAX_PACKET];
        let (n, _) = host.recv_from(&mut buf).unwrap();
        let Some(Message::Challenge(cookie)) = unpack(&buf[..n]) else {
            panic!("challenge missing")
        };
        let claim = packet(&Message::Claim(cookie, sample()), 0).unwrap();
        dir.receive(&directory_socket, udp().local_addr().unwrap(), &claim, now);
        assert!(dir.registered.is_empty());
        dir.receive(
            &directory_socket,
            from,
            &packet(&Message::Claim(cookie ^ 1, sample()), 0).unwrap(),
            now,
        );
        assert!(dir.registered.is_empty());
        dir.receive(&directory_socket, from, &claim, now);
        assert!(dir.registered.contains_key(&from));
        dir.expire(now + LISTING_TTL);
        dir.receive(&directory_socket, from, &claim, now + LISTING_TTL);
        assert!(dir.registered.is_empty());
    }

    #[test]
    fn disabled_public_listing_still_serves_lan_and_ignores_directory_challenges() {
        let game = udp();
        let browser = udp();
        let to = browser.local_addr().unwrap();
        let mut publisher = Publisher::new(None);
        publisher.heartbeat(&game, Instant::now(), &sample());
        publisher.receive(
            &game,
            to,
            &packet(&Message::Challenge(3), 0).unwrap(),
            &sample(),
        );
        publisher.receive(
            &game,
            to,
            &packet(&Message::Probe(7), QUERY_SIZE).unwrap(),
            &sample(),
        );
        let mut buf = [0; MAX_PACKET];
        let (n, address) = browser.recv_from(&mut buf).unwrap();
        assert_eq!(address, game.local_addr().unwrap());
        assert!(
            matches!(unpack(&buf[..n]), Some(Message::LanHost(7, info)) if info.name == "Local Club")
        );
        assert!(!publisher.awaiting_challenge);
    }
}

use alloc::boxed::Box;

use collections::Vec;
use collections::string::ToString;

use common::random::rand;

use core::{cmp, mem, ptr, slice, str};

use fs::{KScheme, Resource, Url};

use network::common::{n16, Checksum, Ipv4Addr, IP_ADDR, FromBytes, ToBytes};

use system::error::{Error, Result, ENOENT};

#[derive(Copy, Clone, Debug)]
#[repr(packed)]
pub struct UdpHeader {
    pub src: n16,
    pub dst: n16,
    pub len: n16,
    pub checksum: Checksum,
}

#[derive(Debug)]
pub struct Udp {
    pub header: UdpHeader,
    pub data: Vec<u8>,
}

impl FromBytes for Udp {
    fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        if bytes.len() >= mem::size_of::<UdpHeader>() {
            unsafe {
                Option::Some(Udp {
                    header: ptr::read(bytes.as_ptr() as *const UdpHeader),
                    data: bytes[mem::size_of::<UdpHeader>()..bytes.len()].to_vec(),
                })
            }
        } else {
            Option::None
        }
    }
}

impl ToBytes for Udp {
    fn to_bytes(&self) -> Vec<u8> {
        unsafe {
            let header_ptr: *const UdpHeader = &self.header;
            let mut ret = Vec::from(slice::from_raw_parts(header_ptr as *const u8,
                                                          mem::size_of::<UdpHeader>()));
            ret.extend_from_slice(&self.data);
            ret
        }
    }
}

/// UDP resource
pub struct UdpResource {
    ip: Box<Resource>,
    data: Vec<u8>,
    peer_addr: Ipv4Addr,
    peer_port: u16,
    host_port: u16,
}

impl Resource for UdpResource {
    fn dup(&self) -> Result<Box<Resource>> {
        debugln!("Dup udp");
        match self.ip.dup() {
            Ok(ip) => {
                Ok(Box::new(UdpResource {
                    ip: ip,
                    data: self.data.clone(),
                    peer_addr: self.peer_addr,
                    peer_port: self.peer_port,
                    host_port: self.host_port,
                }))
            }
            Err(err) => Err(err),
        }
    }

    fn path(&self, buf: &mut [u8]) -> Result<usize> {
        let path_string = format!("udp:{}:{}/{}", self.peer_addr.to_string(), self.peer_port, self.host_port);
        let path = path_string.as_bytes();

        for (b, p) in buf.iter_mut().zip(path.iter()) {
            *b = *p;
        }

        Ok(cmp::min(buf.len(), path.len()))
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        debugln!("  Read udp:{}:{}/{}: {}", self.peer_addr.to_string(), self.peer_port, self.host_port, buf.len());

        while self.data.is_empty() {
            let mut bytes = [0; 65536];
            debugln!("  Attempt read from parent udp:{}:{}/{}: {}", self.peer_addr.to_string(), self.peer_port, self.host_port, bytes.len());
            let count = try!(self.ip.read(&mut bytes));
            debugln!("  Read from parent udp:{}:{}/{}: {}", self.peer_addr.to_string(), self.peer_port, self.host_port, count);
            if let Some(datagram) = Udp::from_bytes(bytes[.. count].to_vec()) {
                if datagram.header.dst.get() == self.host_port && datagram.header.src.get() == self.peer_port {
                    self.data = datagram.data;
                    break;
                } else {
                    debugln!("  Ignore udp:{}:{}/{}: {}", self.peer_addr.to_string(), self.peer_port, self.host_port, count);
                }
            }
        }

        // TODO: Allow splitting
        let mut i = 0;
        while i < buf.len() && i < self.data.len() {
            buf[i] = self.data[i];
            i += 1;
        }

        self.data.clear();

        debugln!("  Return udp:{}:{}/{}: {}", self.peer_addr.to_string(), self.peer_port, self.host_port, i);
        return Ok(i);
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        debugln!("  Write udp:{}:{}/{}: {}", self.peer_addr.to_string(), self.peer_port, self.host_port, buf.len());

        let udp_data = Vec::from(buf);

        let mut udp = Udp {
            header: UdpHeader {
                src: n16::new(self.host_port),
                dst: n16::new(self.peer_port),
                len: n16::new((mem::size_of::<UdpHeader>() + udp_data.len()) as u16),
                checksum: Checksum { data: 0 },
            },
            data: udp_data,
        };

        unsafe {
            let proto = n16::new(0x11);
            let datagram_len = n16::new((mem::size_of::<UdpHeader>() + udp.data.len()) as u16);
            udp.header.checksum.data =
                Checksum::compile(Checksum::sum((&IP_ADDR as *const Ipv4Addr) as usize,
                                                mem::size_of::<Ipv4Addr>()) +
                                  Checksum::sum((&self.peer_addr as *const Ipv4Addr) as usize,
                                                mem::size_of::<Ipv4Addr>()) +
                                  Checksum::sum((&proto as *const n16) as usize,
                                                mem::size_of::<n16>()) +
                                  Checksum::sum((&datagram_len as *const n16) as usize,
                                                mem::size_of::<n16>()) +
                                  Checksum::sum((&udp.header as *const UdpHeader) as usize,
                                                mem::size_of::<UdpHeader>()) +
                                  Checksum::sum(udp.data.as_ptr() as usize, udp.data.len()));
        }

        self.ip.write(&udp.to_bytes()).and(Ok(buf.len()))
    }

    fn sync(&mut self) -> Result<()> {
        self.ip.sync()
    }
}

impl Drop for UdpResource {
    fn drop(&mut self) {
        debugln!("Drop udp");
    }
}

/// UDP UdpScheme
pub struct UdpScheme;

impl KScheme for UdpScheme {
    fn scheme(&self) -> &str {
        "udp"
    }

    fn open(&mut self, url: Url, _: usize) -> Result<Box<Resource>> {
        let mut parts = url.reference().split('/');
        let remote = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("");

        // Check host and port vs path
        if ! path.is_empty() {
            let mut remote_parts = remote.split(':');
            let host_port = remote_parts.nth(1).unwrap_or("").parse::<usize>().unwrap_or(0);
            if host_port > 0 && host_port < 65536 {
                debugln!("Wait for incoming UDP: {}", url.reference());
                while let Ok(mut ip) = Url::from_str("ip:/11").unwrap().open() {
                    debugln!("Accept incoming UDP: {}", url.reference());
                    let mut bytes = [0; 65536];
                    match ip.read(&mut bytes) {
                        Ok(count) => {
                            debugln!("Incoming UDP {}: {}", url.reference(), count);
                            if let Some(datagram) = Udp::from_bytes(bytes[.. count].to_vec()) {
                                if datagram.header.dst.get() as usize == host_port {
                                    let mut path = [0; 256];
                                    if let Ok(path_count) = ip.path(&mut path) {
                                        let ip_reference = unsafe { str::from_utf8_unchecked(&path[.. path_count]) }.split(':').nth(1).unwrap_or("");
                                        let ip_remote = ip_reference.split('/').next().unwrap_or("");
                                        let peer_addr = ip_remote.split(':').next().unwrap_or("");

                                        return Ok(Box::new(UdpResource {
                                            ip: ip,
                                            data: datagram.data,
                                            peer_addr: Ipv4Addr::from_string(&peer_addr.to_string()),
                                            peer_port: datagram.header.src.get(),
                                            host_port: host_port as u16,
                                        }));
                                    }
                                }else{
                                    debugln!("Incoming UDP {}: Not a matching UDP Packet", url.reference());
                                }
                            } else {
                                debugln!("Incoming UDP {}: Not a valid UDP Packet", url.reference());
                            }
                        },
                        Err(_) => break
                    }
                }
                debugln!("Failed to get incoming UDP: {}", url.reference());
            } else {
                debugln!("Invalid port for incoming UDP: {}", host_port);
            }
        } else {
            let mut remote_parts = remote.split(':');
            let peer_addr = remote_parts.next().unwrap_or("");
            let peer_port = remote_parts.next().unwrap_or("").parse::<u16>().unwrap_or(0);
            if peer_port > 0 {
                let host_port = (rand() % 32768 + 32768) as u16;

                if let Ok(ip) = Url::from_str(&format!("ip:{}/11", peer_addr)).unwrap().open() {
                    debugln!("Open UDP: {}", url.reference());
                    return Ok(Box::new(UdpResource {
                        ip: ip,
                        data: Vec::new(),
                        peer_addr: Ipv4Addr::from_string(&peer_addr.to_string()),
                        peer_port: peer_port,
                        host_port: host_port,
                    }));
                }
            }
        }

        Err(Error::new(ENOENT))
    }
}

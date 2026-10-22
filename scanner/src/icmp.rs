//! ICMP echo request and echo reply, the two messages `ping` is made of
//! (RFC 792), built and read by hand.
//!
//! An echo message is an 8-byte header followed by any data, which the
//! other side sends back unchanged in its reply:
//!
//! ```text
//!  0        7 8       15 16                     31   bit
//! +----------+----------+-------------------------+
//! |   type   |   code   |        checksum         |   type 8 = request, 0 = reply; code 0
//! +----------+----------+-------------------------+
//! |      identifier     |     sequence number     |   chosen by the sender, echoed back
//! +---------------------+-------------------------+
//! |  data ...                                     |   echoed back too
//! ```
//!
//! Fields wider than a byte are big-endian ("network byte order"):
//! `u16::to_be_bytes` / `from_be_bytes` do what Python's
//! `struct.pack("!BBHHH", ...)` / `struct.unpack` would.

/// `type` of an echo reply.
pub const ECHO_REPLY: u8 = 0;
/// `type` of "destination unreachable": a router (or the host) could not
/// deliver the packet; the code says why (3 = the port is closed).
pub const DESTINATION_UNREACHABLE: u8 = 3;
/// `type` of an echo request.
pub const ECHO_REQUEST: u8 = 8;
/// `type` of "time exceeded": a router dropped the packet because its TTL
/// ran out there. What traceroute listens for.
pub const TIME_EXCEEDED: u8 = 11;
/// Bytes before the data.
pub const HEADER_LEN: usize = 8;

/// The parts of an echo message the sweep cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Echo {
    pub identifier: u16,
    pub sequence: u16,
    pub data: Vec<u8>,
}

impl Echo {
    /// This message as the bytes of an echo request, checksum filled in.
    pub fn request_bytes(&self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(HEADER_LEN + self.data.len());
        packet.push(ECHO_REQUEST); // type
        packet.push(0); // code
        packet.extend_from_slice(&[0, 0]); // checksum: computed over the packet with this field zero
        packet.extend_from_slice(&self.identifier.to_be_bytes());
        packet.extend_from_slice(&self.sequence.to_be_bytes());
        packet.extend_from_slice(&self.data);
        let sum = checksum(&packet);
        packet[2..4].copy_from_slice(&sum.to_be_bytes());
        packet
    }

    /// Read an echo reply that starts at the ICMP header. `None` for anything
    /// else: other ICMP messages, requests (a raw socket sees those too),
    /// packets too short to be an echo, or damaged ones.
    pub fn parse_reply(packet: &[u8]) -> Option<Echo> {
        if packet.len() < HEADER_LEN || packet[0] != ECHO_REPLY || packet[1] != 0 {
            return None;
        }
        if checksum(packet) != 0 {
            return None; // see `checksum`: an intact message sums to zero
        }
        Some(Echo {
            identifier: u16::from_be_bytes([packet[4], packet[5]]),
            sequence: u16::from_be_bytes([packet[6], packet[7]]),
            data: packet[HEADER_LEN..].to_vec(),
        })
    }
}

/// The Internet checksum (RFC 1071): add the message up as 16-bit
/// big-endian words (an odd last byte is padded with a zero), fold every
/// carry out of the top 16 bits back into the bottom, and flip all the bits.
/// Run over a whole message, its own checksum field included, it gives 0
/// when nothing was damaged on the way.
pub fn checksum(bytes: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for pair in bytes.chunks(2) {
        let low = pair.get(1).copied().unwrap_or(0);
        sum += u32::from(u16::from_be_bytes([pair[0], low]));
    }
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// A raw socket delivers the IPv4 header in front of the ICMP message; this
/// skips it. The header's length is the low four bits of its first byte,
/// counted in 32-bit words (5, so 20 bytes, without options); the high four
/// bits are the IP version.
pub fn strip_ipv4_header(packet: &[u8]) -> Option<&[u8]> {
    let first = *packet.first()?;
    if first >> 4 != 4 {
        return None;
    }
    let header_len = usize::from(first & 0x0F) * 4;
    packet.get(header_len..)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_matches_the_rfc_1071_example() {
        // RFC 1071, section 3: these words sum to 0xddf2 after folding.
        let bytes = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(checksum(&bytes), !0xddf2);
        // An odd length is padded with a zero byte.
        assert_eq!(checksum(&[0x01]), checksum(&[0x01, 0x00]));
    }

    #[test]
    fn a_request_has_its_fields_in_network_order_and_sums_to_zero() {
        let echo = Echo {
            identifier: 0x1234,
            sequence: 0x0102,
            data: b"abc".to_vec(),
        };
        let packet = echo.request_bytes();
        assert_eq!(packet.len(), HEADER_LEN + 3);
        assert_eq!(&packet[..2], &[ECHO_REQUEST, 0]);
        assert_eq!(&packet[4..], &[0x12, 0x34, 0x01, 0x02, b'a', b'b', b'c']);
        assert_eq!(
            checksum(&packet),
            0,
            "the checksum makes the whole packet sum to zero"
        );
    }

    /// What a host sends back: the request with type 0 and a new checksum.
    fn reply_to(request: &[u8]) -> Vec<u8> {
        let mut reply = request.to_vec();
        reply[0] = ECHO_REPLY;
        reply[2..4].copy_from_slice(&[0, 0]);
        let sum = checksum(&reply);
        reply[2..4].copy_from_slice(&sum.to_be_bytes());
        reply
    }

    #[test]
    fn replies_parse_and_everything_else_does_not() {
        let echo = Echo {
            identifier: 7,
            sequence: 42,
            data: vec![9; 12],
        };
        let request = echo.request_bytes();
        let reply = reply_to(&request);
        assert_eq!(Echo::parse_reply(&reply), Some(echo));

        assert_eq!(
            Echo::parse_reply(&request),
            None,
            "a request is not a reply"
        );
        assert_eq!(Echo::parse_reply(&reply[..7]), None, "too short");
        let mut damaged = reply.clone();
        damaged[10] ^= 0xFF;
        assert_eq!(Echo::parse_reply(&damaged), None, "the checksum catches it");
        let mut unreachable = reply;
        unreachable[0] = 3; // destination unreachable
        assert_eq!(Echo::parse_reply(&unreachable), None);
    }

    #[test]
    fn the_ipv4_header_is_skipped_by_its_own_length() {
        let mut packet = vec![0x45]; // version 4, 5 words
        packet.extend([0; 19]);
        packet.extend([ECHO_REPLY, 0]);
        assert_eq!(strip_ipv4_header(&packet), Some(&[ECHO_REPLY, 0][..]));

        let mut with_options = vec![0x46]; // 6 words: 4 bytes of options
        with_options.extend([0; 23]);
        with_options.push(0xAB);
        assert_eq!(strip_ipv4_header(&with_options), Some(&[0xAB][..]));

        assert_eq!(strip_ipv4_header(&[0x60, 0, 0]), None, "IPv6");
        assert_eq!(
            strip_ipv4_header(&[0x45, 0]),
            None,
            "shorter than its header"
        );
        assert_eq!(strip_ipv4_header(&[]), None);
    }
}

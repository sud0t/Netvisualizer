//! Where each host goes in the scene when the map is built from a scan: the
//! scanner at the origin, everyone else on concentric rings around it in
//! address order, the gateway first. y is up; each ring floats a little
//! higher than the one inside it. (Moved here from `graphics/layout.py`.)

use std::f32::consts::TAU;
use std::net::IpAddr;

use crate::map::{Host, Role};

/// World units between neighbours on a ring.
pub const SPACING: f32 = 3.0;
/// Radius of the innermost ring.
pub const MIN_RADIUS: f32 = 8.0;
/// Radial distance between rings.
pub const RING_GAP: f32 = 6.0;
/// How much higher each ring is than the one inside it.
pub const RING_RISE: f32 = 1.5;

/// How many hosts go on each ring, innermost first: as many as fit with
/// [`SPACING`] between them, the last ring taking the rest.
pub fn ring_plan(count: usize) -> Vec<usize> {
    let mut plan = Vec::new();
    let mut remaining = count;
    while remaining > 0 {
        let radius = MIN_RADIUS + plan.len() as f32 * RING_GAP;
        let capacity = ((TAU * radius / SPACING).floor() as usize).max(1);
        let take = capacity.min(remaining);
        plan.push(take);
        remaining -= take;
    }
    plan
}

/// A position for every host, parallel to `hosts`: the first scanner at the
/// origin, the others (a second scanner too, should one turn up) on the
/// rings. Odd rings are turned by half a step so nodes do not line up
/// radially. Coordinates are rounded to 1/1000 to keep saved maps tidy.
pub fn star_ring(hosts: &[Host]) -> Vec<[f32; 3]> {
    let mut positions = vec![[0.0; 3]; hosts.len()];
    let centre = hosts.iter().position(|h| h.role() == Role::Scanner);
    let mut others: Vec<usize> = (0..hosts.len()).filter(|&i| Some(i) != centre).collect();
    // The gateway first, then by address (IPv4 before IPv6, numerically, so
    // .9 before .10); ids that are not addresses go last, in their own order.
    others.sort_by_key(|&i| {
        let host = &hosts[i];
        let address = host.id.parse::<IpAddr>().ok().map(|ip| match ip {
            IpAddr::V4(v4) => (4, u128::from(v4.to_bits())),
            IpAddr::V6(v6) => (6, v6.to_bits()),
        });
        (host.role() != Role::Gateway, address.is_none(), address)
    });

    let mut next = others.iter();
    for (ring, count) in ring_plan(others.len()).into_iter().enumerate() {
        let radius = MIN_RADIUS + ring as f32 * RING_GAP;
        let y = ring as f32 * RING_RISE;
        let step = TAU / count as f32;
        let offset = if ring % 2 == 1 { step / 2.0 } else { 0.0 };
        for slot in 0..count {
            let angle = offset + slot as f32 * step;
            let index = *next.next().expect("the plan places every host");
            positions[index] = [
                round3(radius * angle.cos()),
                y,
                round3(radius * angle.sin()),
            ];
        }
    }
    positions
}

fn round3(value: f32) -> f32 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hosts with these ids; `scanner` and `gateway` name the ones with those roles.
    fn hosts(ids: &[&str], scanner: Option<&str>, gateway: Option<&str>) -> Vec<Host> {
        ids.iter()
            .map(|id| {
                let role = if Some(*id) == scanner {
                    "scanner"
                } else if Some(*id) == gateway {
                    "gateway"
                } else {
                    "host"
                };
                serde_json::from_value(serde_json::json!({"id": id, "role": role})).unwrap()
            })
            .collect()
    }

    fn lan(n: usize) -> Vec<String> {
        (2..2 + n)
            .map(|i| format!("10.0.{}.{}", i / 256, i % 256))
            .collect()
    }

    fn with_scanner(n: usize) -> Vec<Host> {
        let mut ids = vec!["10.0.0.1".to_string()];
        ids.extend(lan(n));
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        hosts(&ids, Some("10.0.0.1"), None)
    }

    fn radius([x, _, z]: [f32; 3]) -> f32 {
        x.hypot(z)
    }

    #[test]
    fn scanner_at_the_origin_and_everyone_somewhere_else() {
        let positions = star_ring(&with_scanner(10));
        assert_eq!(positions[0], [0.0, 0.0, 0.0]);
        for (i, a) in positions.iter().enumerate() {
            for b in &positions[i + 1..] {
                assert_ne!(a, b, "two hosts share a spot");
            }
        }
    }

    #[test]
    fn a_small_network_sits_on_the_inner_ring() {
        for p in &star_ring(&with_scanner(6))[1..] {
            assert!((radius(*p) - MIN_RADIUS).abs() < 0.01, "{p:?}");
            assert_eq!(p[1], 0.0);
        }
    }

    #[test]
    fn a_large_network_spills_onto_further_rings() {
        let positions = star_ring(&with_scanner(60));
        let mut radii: Vec<i32> = positions[1..]
            .iter()
            .map(|p| radius(*p).round() as i32)
            .collect();
        radii.dedup();
        assert!(radii.len() > 1, "{radii:?}");
        assert!(
            positions[1..].iter().any(|p| p[1] == RING_RISE),
            "outer rings float higher"
        );
    }

    #[test]
    fn the_gateway_comes_first_on_the_inner_ring() {
        let ids = [
            "10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5", "10.0.0.6",
        ];
        let positions = star_ring(&hosts(&ids, Some("10.0.0.1"), Some("10.0.0.6")));
        let [x, _, z] = positions[5];
        assert!(
            (x - MIN_RADIUS).abs() < 0.01 && z.abs() < 0.01,
            "{:?}",
            positions[5]
        );
    }

    #[test]
    fn hosts_go_around_the_ring_in_address_order() {
        // Listed out of order, and .10 after .9 numerically, not as text.
        let ids = ["10.0.0.1", "10.0.0.10", "10.0.0.3", "10.0.0.9", "10.0.0.2"];
        let positions = star_ring(&hosts(&ids, Some("10.0.0.1"), None));
        let angle = |i: usize| positions[i][2].atan2(positions[i][0]).rem_euclid(TAU);
        let order = [4, 2, 3, 1]; // .2, .3, .9, .10
        for pair in order.windows(2) {
            assert!(angle(pair[0]) < angle(pair[1]), "{positions:?}");
        }
    }

    #[test]
    fn without_a_scanner_nobody_takes_the_origin() {
        let ids = ["10.0.0.2", "10.0.0.3", "10.0.0.4"];
        for p in star_ring(&hosts(&ids, None, None)) {
            assert!((radius(p) - MIN_RADIUS).abs() < 0.01, "{p:?}");
        }
        assert!(star_ring(&[]).is_empty());
    }

    #[test]
    fn ring_plans_add_up_and_fill_the_inner_ring_first() {
        let inner = ring_plan(1000)[0];
        assert_eq!(inner, 16, "2π·8/3 rounded down");
        for n in [0, 1, 5, 16, 17, 60, 254] {
            let plan = ring_plan(n);
            assert_eq!(plan.iter().sum::<usize>(), n);
            if plan.len() > 1 {
                assert_eq!(plan[0], inner);
            }
        }
    }
}

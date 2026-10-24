//! Where each host goes in the scene when the map is built from a scan. The
//! layout follows the links, as a tree of rings:
//!
//! - the scanner at the origin;
//! - the hosts linked to it (its own subnet) on concentric rings around it,
//!   the gateway first, then in address order;
//! - the hosts linked to one of those (hosts behind the gateway, or behind
//!   a router traceroute found) on rings of their own around a point
//!   straight out beyond the host they hang off, outside every ring placed
//!   before, so their links fan out away from the middle; and so on, level
//!   by level, for as deep as the paths go.
//!
//! y is up; every ring lies on the floor (y = 0), where the nodes stand.
//! (Moved here from `graphics/layout.py`.)

use std::collections::{HashMap, VecDeque};
use std::f32::consts::TAU;
use std::net::IpAddr;

use crate::map::{Host, Link, Role};

/// World units between neighbours on a ring.
pub const SPACING: f32 = 3.0;
/// Radius of the innermost ring.
pub const MIN_RADIUS: f32 = 8.0;
/// Radial distance between rings.
pub const RING_GAP: f32 = 6.0;

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

/// A position for every host, parallel to `hosts` (see the module comment).
/// Each host hangs off the source of the first link that targets it; hosts
/// no link reaches go on the scanner's rings, as do all hosts of a map
/// without a scanner. Coordinates are rounded to 1/1000 to keep saved maps
/// tidy.
pub fn star_ring(hosts: &[Host], links: &[Link]) -> Vec<[f32; 3]> {
    let mut positions = vec![[0.0; 3]; hosts.len()];
    let centre = hosts.iter().position(|h| h.role() == Role::Scanner);
    let index: HashMap<&str, usize> = hosts
        .iter()
        .enumerate()
        .map(|(i, host)| (host.id.as_str(), i))
        .collect();
    let mut parent: Vec<Option<usize>> = vec![None; hosts.len()];
    for link in links {
        if let (Some(&source), Some(&target)) = (
            index.get(link.source.as_str()),
            index.get(link.target.as_str()),
        ) && source != target
            && parent[target].is_none()
        {
            parent[target] = Some(source);
        }
    }

    // The scanner's rings.
    let around_centre: Vec<usize> = (0..hosts.len())
        .filter(|&i| Some(i) != centre && (parent[i].is_none() || parent[i] == centre))
        .collect();
    let mut placed = vec![false; hosts.len()];
    if let Some(centre) = centre {
        placed[centre] = true;
    }
    // How far out the rings placed so far reach: every group goes beyond it.
    let mut frontier = place_on_rings(hosts, &around_centre, [0.0; 3], &mut positions);

    // Then outward, a level at a time: the hosts hanging off a placed host
    // get rings of their own beyond it, and may have hosts of their own.
    let mut queue: VecDeque<usize> = around_centre.iter().copied().collect();
    for &i in &around_centre {
        placed[i] = true;
    }
    while let Some(via) = queue.pop_front() {
        let members: Vec<usize> = (0..hosts.len())
            .filter(|&i| parent[i] == Some(via) && !placed[i])
            .collect();
        if members.is_empty() {
            continue;
        }
        let [x, _, z] = positions[via];
        let out = x.hypot(z);
        let (dx, dz) = if out > 0.0 {
            (x / out, z / out)
        } else {
            (1.0, 0.0)
        };
        // Far enough out that the group's outermost ring clears everything
        // placed so far and the host it hangs off, by one gap plus the
        // group's own reach.
        let reach = MIN_RADIUS + (ring_plan(members.len()).len() - 1) as f32 * RING_GAP;
        let distance = frontier.max(out) + RING_GAP + reach;
        let anchor = [dx * distance, 0.0, dz * distance];
        place_on_rings(hosts, &members, anchor, &mut positions);
        frontier = distance + reach;
        for &i in &members {
            placed[i] = true;
            queue.push_back(i);
        }
    }
    // Hosts in a loop of links that never reaches the scanner: on a ring
    // of their own, beyond everything.
    let leftover: Vec<usize> = (0..hosts.len()).filter(|&i| !placed[i]).collect();
    if !leftover.is_empty() {
        let reach = MIN_RADIUS + (ring_plan(leftover.len()).len() - 1) as f32 * RING_GAP;
        let anchor = [frontier + RING_GAP + reach, 0.0, 0.0];
        place_on_rings(hosts, &leftover, anchor, &mut positions);
    }
    positions
}

/// Put `members` on concentric rings around `centre`: the gateway first,
/// then by address (IPv4 before IPv6, numerically, so .9 before .10; ids
/// that are not addresses last, in their own order). Odd rings are turned
/// by half a step so nodes do not line up radially. Returns the radius of
/// the outermost ring, 0 when there is nobody to place.
fn place_on_rings(
    hosts: &[Host],
    members: &[usize],
    centre: [f32; 3],
    positions: &mut [[f32; 3]],
) -> f32 {
    let mut order = members.to_vec();
    order.sort_by_key(|&i| {
        let host = &hosts[i];
        let address = host.id.parse::<IpAddr>().ok().map(|ip| match ip {
            IpAddr::V4(v4) => (4, u128::from(v4.to_bits())),
            IpAddr::V6(v6) => (6, v6.to_bits()),
        });
        (host.role() != Role::Gateway, address.is_none(), address)
    });

    let mut next = order.iter();
    let mut outermost = 0.0;
    for (ring, count) in ring_plan(order.len()).into_iter().enumerate() {
        let radius = MIN_RADIUS + ring as f32 * RING_GAP;
        outermost = radius;
        let step = TAU / count as f32;
        let offset = if ring % 2 == 1 { step / 2.0 } else { 0.0 };
        for slot in 0..count {
            let angle = offset + slot as f32 * step;
            let index = *next.next().expect("the plan places every host");
            positions[index] = [
                round3(centre[0] + radius * angle.cos()),
                centre[1],
                round3(centre[2] + radius * angle.sin()),
            ];
        }
    }
    outermost
}

fn round3(value: f32) -> f32 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

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

    fn link(source: &str, target: &str) -> Link {
        Link {
            source: source.to_string(),
            target: target.to_string(),
            kind: None,
            rest: Map::new(),
        }
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
        let positions = star_ring(&with_scanner(10), &[]);
        assert_eq!(positions[0], [0.0, 0.0, 0.0]);
        for (i, a) in positions.iter().enumerate() {
            for b in &positions[i + 1..] {
                assert_ne!(a, b, "two hosts share a spot");
            }
        }
    }

    #[test]
    fn a_small_network_sits_on_the_inner_ring() {
        for p in &star_ring(&with_scanner(6), &[])[1..] {
            assert!((radius(*p) - MIN_RADIUS).abs() < 0.01, "{p:?}");
            assert_eq!(p[1], 0.0);
        }
    }

    #[test]
    fn a_large_network_spills_onto_further_rings() {
        let positions = star_ring(&with_scanner(60), &[]);
        let mut radii: Vec<i32> = positions[1..]
            .iter()
            .map(|p| radius(*p).round() as i32)
            .collect();
        radii.dedup();
        assert!(radii.len() > 1, "{radii:?}");
        assert!(
            positions.iter().all(|p| p[1] == 0.0),
            "every ring lies on the floor"
        );
    }

    #[test]
    fn the_gateway_comes_first_on_the_inner_ring() {
        let ids = [
            "10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5", "10.0.0.6",
        ];
        let positions = star_ring(&hosts(&ids, Some("10.0.0.1"), Some("10.0.0.6")), &[]);
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
        let positions = star_ring(&hosts(&ids, Some("10.0.0.1"), None), &[]);
        let angle = |i: usize| positions[i][2].atan2(positions[i][0]).rem_euclid(TAU);
        let order = [4, 2, 3, 1]; // .2, .3, .9, .10
        for pair in order.windows(2) {
            assert!(angle(pair[0]) < angle(pair[1]), "{positions:?}");
        }
    }

    #[test]
    fn without_a_scanner_nobody_takes_the_origin() {
        let ids = ["10.0.0.2", "10.0.0.3", "10.0.0.4"];
        for p in star_ring(&hosts(&ids, None, None), &[]) {
            assert!((radius(p) - MIN_RADIUS).abs() < 0.01, "{p:?}");
        }
        assert_eq!(star_ring(&[], &[]), Vec::<[f32; 3]>::new());
    }

    #[test]
    fn hosts_behind_the_gateway_get_rings_of_their_own_beyond_it() {
        // The scanner, its gateway and 19 neighbours (two rings: 16 + 3, out
        // to radius 14), and 5 hosts on another network behind the gateway.
        let mut ids = vec!["10.0.0.1".to_string(), "10.0.0.254".to_string()];
        ids.extend(lan(19));
        let remote: Vec<String> = (1..=5).map(|i| format!("192.168.5.{i}")).collect();
        ids.extend(remote.iter().cloned());
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let map_hosts = hosts(&ids, Some("10.0.0.1"), Some("10.0.0.254"));
        let mut links: Vec<Link> = ids[1..21].iter().map(|id| link("10.0.0.1", id)).collect();
        links.extend(remote.iter().map(|id| link("10.0.0.254", id)));

        let positions = star_ring(&map_hosts, &links);
        let gateway = positions[1];
        assert!((gateway[0] - MIN_RADIUS).abs() < 0.01 && gateway[2].abs() < 0.01);
        let local_reach = MIN_RADIUS + RING_GAP;
        for (id, p) in ids.iter().zip(&positions).skip(21) {
            assert!(
                radius(*p) > local_reach + RING_GAP - 0.01,
                "{id} at {p:?} is inside the scanner's rings"
            );
            assert!(
                p[0] > local_reach,
                "{id} at {p:?} is not out beyond the gateway"
            );
        }
        // The group sits on its own ring around a point on the gateway's side.
        let centre = positions[21..].iter().fold([0.0f32; 2], |acc, p| {
            [acc[0] + p[0] / 5.0, acc[1] + p[2] / 5.0]
        });
        assert!(centre[1].abs() < 0.01, "on the gateway's axis: {centre:?}");
        let expected = local_reach + RING_GAP + MIN_RADIUS;
        assert!((centre[0] - expected).abs() < 0.01, "{centre:?}");
        for p in &positions[21..] {
            let from_centre = (p[0] - centre[0]).hypot(p[2] - centre[1]);
            assert!((from_centre - MIN_RADIUS).abs() < 0.01, "{p:?}");
        }
    }

    /// scanner → gateway → router → router → host: each level further out
    /// along the same direction, nothing overlapping.
    #[test]
    fn a_chain_of_routers_goes_straight_out_level_by_level() {
        let ids = ["10.0.0.1", "10.0.0.254", "10.1.0.1", "10.2.0.1", "10.3.0.7"];
        let map_hosts = hosts(&ids, Some("10.0.0.1"), Some("10.0.0.254"));
        let links = [
            link("10.0.0.1", "10.0.0.254"),
            link("10.0.0.254", "10.1.0.1"),
            link("10.1.0.1", "10.2.0.1"),
            link("10.2.0.1", "10.3.0.7"),
        ];
        let positions = star_ring(&map_hosts, &links);
        for pair in positions.windows(2) {
            assert!(
                radius(pair[1]) > radius(pair[0]) + RING_GAP - 0.01,
                "{positions:?}"
            );
            assert!(
                pair[1][2].abs() < 0.01,
                "on the gateway's axis: {positions:?}"
            );
        }
        // Two routers behind the gateway do not put their hosts on top of
        // each other: the second group lies beyond the first.
        let ids = [
            "10.0.0.1",
            "10.0.0.254",
            "10.1.0.1",
            "10.2.0.1",
            "10.1.0.7",
            "10.2.0.7",
        ];
        let map_hosts = hosts(&ids, Some("10.0.0.1"), Some("10.0.0.254"));
        let links = [
            link("10.0.0.1", "10.0.0.254"),
            link("10.0.0.254", "10.1.0.1"),
            link("10.0.0.254", "10.2.0.1"),
            link("10.1.0.1", "10.1.0.7"),
            link("10.2.0.1", "10.2.0.7"),
        ];
        let positions = star_ring(&map_hosts, &links);
        for (i, a) in positions.iter().enumerate() {
            for b in &positions[i + 1..] {
                let apart = (a[0] - b[0]).hypot(a[2] - b[2]);
                assert!(apart > 1.0, "{positions:?}");
            }
        }
        // A loop that never reaches the scanner still gets a spot.
        let ids = ["10.0.0.1", "10.9.0.1", "10.9.0.2"];
        let map_hosts = hosts(&ids, Some("10.0.0.1"), None);
        let links = [link("10.9.0.1", "10.9.0.2"), link("10.9.0.2", "10.9.0.1")];
        let positions = star_ring(&map_hosts, &links);
        assert_ne!(positions[1], positions[2]);
        assert!(radius(positions[1]) > 0.0 && radius(positions[2]) > 0.0);
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

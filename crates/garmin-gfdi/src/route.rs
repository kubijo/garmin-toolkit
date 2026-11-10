//! Known GATT routes from Gadgetbridge `CommunicatorV1/V2`; no discovery or I/O.

/// One discovered characteristic, supplied by a platform adapter.
/// UUIDs use their conventional hexadecimal numeric representation, not wire order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Characteristic {
    /// UUID of the containing service.
    pub service: u128,
    /// UUID of this characteristic.
    pub uuid: u128,
    /// Supports notifications for incoming protocol bytes.
    pub notify: bool,
    /// Supports writes for outgoing protocol bytes.
    pub write: bool,
}

/// Transport framing required by a known pair of characteristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    /// Multiplexed handles require registration before GFDI traffic.
    MultiLink,
    /// Direct GFDI characteristics, newer legacy UUID family.
    DirectV1,
    /// Direct GFDI characteristics, original UUID family.
    DirectV0,
}

/// A complete GATT route; selecting one never starts a connection or session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    /// Framing/negotiation family.
    pub kind: RouteKind,
    /// UUID of the containing service.
    pub service: u128,
    /// Characteristic the host subscribes to.
    pub receive: u128,
    /// Characteristic the host writes to.
    pub send: u128,
}

impl Route {
    fn available(self, characteristics: &[Characteristic]) -> bool {
        characteristics
            .iter()
            .any(|c| c.service == self.service && c.uuid == self.receive && c.notify)
            && characteristics
                .iter()
                .any(|c| c.service == self.service && c.uuid == self.send && c.write)
    }
}

/// Select a complete known pair with usable properties from a supplied inventory.
///
/// Prefer the first complete `MultiLink` slot, then direct V1 and V0, matching the upstream order.
/// No cross-slot or cross-service pairing is permitted.
/// This preference is not a fallback policy after a failed registration or connection.
#[must_use]
pub fn select_route(characteristics: &[Characteristic]) -> Option<Route> {
    for slot in 0..5_u128 {
        let route = Route {
            kind: RouteKind::MultiLink,
            service: 0x6a4e_2800_667b_11e3_949a_0800_200c_9a66,
            receive: 0x6a4e_2810_667b_11e3_949a_0800_200c_9a66 + (slot << 96),
            send: 0x6a4e_2820_667b_11e3_949a_0800_200c_9a66 + (slot << 96),
        };
        if route.available(characteristics) {
            return Some(route);
        }
    }
    [
        Route {
            kind: RouteKind::DirectV1,
            service: 0x6a4e_2401_667b_11e3_949a_0800_200c_9a66,
            receive: 0x6a4e_cd28_667b_11e3_949a_0800_200c_9a66,
            send: 0x6a4e_4c80_667b_11e3_949a_0800_200c_9a66,
        },
        Route {
            kind: RouteKind::DirectV0,
            service: 0x9b01_2401_bc30_ce9a_e111_0f67_e491_abde,
            receive: 0x4acb_cd28_7425_868e_f447_915c_8f00_d0cb,
            send: 0xdf33_4c80_e6a7_d082_274d_78fc_66f8_5e16,
        },
    ]
    .into_iter()
    .find(|route| route.available(characteristics))
}

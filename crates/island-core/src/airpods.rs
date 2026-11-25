//! Reading AirPods' battery and ear state from what they advertise over
//! Bluetooth LE.
//!
//! Windows reports only one battery level for AirPods, if any. The earbuds
//! themselves keep broadcasting a short "proximity pairing" message in Apple's
//! manufacturer data, which holds both earbuds' and the case's battery, which
//! of them charge, which sit in an ear, and whether the case lid is open. Only
//! one earbud broadcasts at a time, so every value is told relative to the
//! broadcasting one ("this" and "the other") and a flag says which side that
//! is.
//!
//! The layout below is Apple's undocumented format as the open-source
//! community has observed it. Nothing here touches Windows; the watcher hands
//! over the manufacturer data bytes, without the company id.

/// Apple's Bluetooth company id, under which the message is advertised.
pub const APPLE_COMPANY_ID: u16 = 0x004C;

/// The message type of a proximity pairing message.
const PROXIMITY_PAIRING: u8 = 0x07;
/// How many bytes follow the type and length bytes in a proximity pairing
/// message from AirPods. Other lengths come from other devices.
const PROXIMITY_PAIRING_LEN: usize = 25;

/// A battery nibble counts in tens of percent up to this. Anything above,
/// usually 15, means the level is not known, as for a pod that is not there.
const BATTERY_MAX: u8 = 10;

/// The product, from the model id in the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    AirPods1,
    AirPods2,
    AirPods3,
    AirPods4,
    AirPodsPro,
    AirPodsPro2,
    AirPodsPro2UsbC,
    AirPodsPro3,
    AirPodsMax,
    /// Another Apple or Beats product with the same message.
    Other(u16),
}

impl Model {
    fn from_id(id: u16) -> Self {
        match id {
            0x2002 => Self::AirPods1,
            0x200F => Self::AirPods2,
            0x2013 => Self::AirPods3,
            0x2019 => Self::AirPods4,
            0x200E => Self::AirPodsPro,
            0x2014 => Self::AirPodsPro2,
            0x2024 => Self::AirPodsPro2UsbC,
            0x2027 => Self::AirPodsPro3,
            0x200A => Self::AirPodsMax,
            other => Self::Other(other),
        }
    }

    /// The name shown when Windows has none to offer.
    pub fn name(self) -> &'static str {
        match self {
            Self::AirPods1 | Self::AirPods2 | Self::AirPods3 | Self::AirPods4 => "AirPods",
            Self::AirPodsPro | Self::AirPodsPro2 | Self::AirPodsPro2UsbC | Self::AirPodsPro3 => {
                "AirPods Pro"
            }
            Self::AirPodsMax => "AirPods Max",
            Self::Other(_) => "AirPods",
        }
    }
}

/// One earbud.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pod {
    /// In percent, a multiple of ten. None when the earbud does not say.
    pub battery: Option<u8>,
    pub charging: bool,
    /// A charging earbud is in the case, whatever its ear flag says.
    pub in_ear: bool,
}

/// The charging case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Case {
    /// In percent, a multiple of ten. None when no earbud sits in the case,
    /// since the case speaks only through them.
    pub battery: Option<u8>,
    pub charging: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub model: Model,
    pub left: Pod,
    pub right: Pod,
    pub case: Case,
    pub both_in_case: bool,
    /// Which earbud sent this message. Its own values are first-hand; the
    /// other earbud's come through it and may lag behind.
    pub from_left: bool,
    /// Whether the case lid is open. Only known while both earbuds are in the
    /// case; otherwise the bit means nothing.
    pub lid_open: Option<bool>,
}

impl Status {
    pub fn both_in_ear(&self) -> bool {
        self.left.in_ear && self.right.in_ear
    }
}

/// The AirPods status in Apple's manufacturer data, if it holds one.
///
/// The data is a run of messages, each a type byte, a length byte and that
/// many bytes, so the proximity pairing message is looked for among them.
pub fn parse(data: &[u8]) -> Option<Status> {
    let mut rest = data;
    while let [kind, len, tail @ ..] = rest {
        let len = usize::from(*len);
        let body = tail.get(..len)?;
        if *kind == PROXIMITY_PAIRING
            && let Ok(body) = body.try_into()
        {
            return Some(parse_proximity_pairing(body));
        }
        rest = &tail[len..];
    }
    None
}

/// Reads the body of a proximity pairing message.
fn parse_proximity_pairing(body: &[u8; PROXIMITY_PAIRING_LEN]) -> Status {
    // body[0] is a prefix whose meaning is not known.
    // body[7] is the colour, body[8] is not known, the last 16 bytes are
    // encrypted. None of them are read.
    let model = Model::from_id(u16::from_le_bytes([body[1], body[2]]));
    let flags = body[3];
    let batteries = body[4];
    let charging = body[5];
    let lid = body[6];

    let this_in_ear = flags & 0b0000_0010 != 0;
    let both_in_case = flags & 0b0000_0100 != 0;
    let other_in_ear = flags & 0b0000_1000 != 0;
    let from_left = flags & 0b0010_0000 != 0;

    let this_charging = charging & 0b0001_0000 != 0;
    let other_charging = charging & 0b0010_0000 != 0;

    // An earbud charging in the case can still raise its ear flag, so
    // charging overrides it. This is observed behaviour, not a documented bit.
    let this = Pod {
        battery: battery(batteries & 0x0F),
        charging: this_charging,
        in_ear: this_in_ear && !this_charging,
    };
    let other = Pod {
        battery: battery(batteries >> 4),
        charging: other_charging,
        in_ear: other_in_ear && !other_charging,
    };
    let (left, right) = if from_left {
        (this, other)
    } else {
        (other, this)
    };

    Status {
        model,
        left,
        right,
        case: Case {
            battery: battery(charging & 0x0F),
            charging: charging & 0b0100_0000 != 0,
        },
        both_in_case,
        from_left,
        lid_open: both_in_case.then_some(lid & 0b0000_1000 == 0),
    }
}

fn battery(nibble: u8) -> Option<u8> {
    (nibble <= BATTERY_MAX).then_some(nibble * 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A proximity pairing message with the given model id and the four
    /// bytes after it: flags, batteries, charging and case, lid.
    fn message(model: u16, flags: u8, batteries: u8, charging: u8, lid: u8) -> Vec<u8> {
        let [low, high] = model.to_le_bytes();
        let mut data = vec![
            PROXIMITY_PAIRING,
            PROXIMITY_PAIRING_LEN as u8,
            0x01,
            low,
            high,
        ];
        data.extend([flags, batteries, charging, lid, 0x00, 0x00]);
        data.extend([0xAA; 16]);
        data
    }

    #[test]
    fn reads_both_earbuds_in_ear_from_the_left() {
        // Constructed bytes for AirPods Pro being worn: left broadcasts, both
        // in an ear at 90 %, no earbud in the case.
        let data = message(0x200E, 0x2B, 0x99, 0x8F, 0x01);
        let status = parse(&data).unwrap();
        assert_eq!(status.model, Model::AirPodsPro);
        let worn = Pod {
            battery: Some(90),
            charging: false,
            in_ear: true,
        };
        assert_eq!(status.left, worn);
        assert_eq!(status.right, worn);
        assert_eq!(
            status.case,
            Case {
                battery: None,
                charging: false
            }
        );
        assert!(!status.both_in_case);
        assert_eq!(status.lid_open, None);
        assert!(status.both_in_ear());
    }

    #[test]
    fn unread_bits_and_bytes_change_nothing() {
        let plain = parse(&message(0x200E, 0x2B, 0x99, 0x0F, 0x01)).unwrap();
        // Set the unread status bits (0, 4, 6, 7), bit 7 of the charging
        // byte, the lid's upper bits, the prefix and every byte after the lid.
        let mut noisy = message(0x200E, 0x2B | 0b1101_0001, 0x99, 0x8F, 0xF1);
        noisy[2] = 0xFF;
        for byte in &mut noisy[9..] {
            *byte = 0xFF;
        }
        assert_eq!(parse(&noisy), Some(plain));
    }

    #[test]
    fn the_broadcasting_side_decides_which_values_are_left() {
        // This earbud at 30 % in an ear, the other at 80 % out of it.
        let from_left = parse(&message(0x2014, 0x22, 0x83, 0x0F, 0)).unwrap();
        assert!(from_left.from_left);
        assert_eq!(from_left.left.battery, Some(30));
        assert!(from_left.left.in_ear);
        assert_eq!(from_left.right.battery, Some(80));
        assert!(!from_left.right.in_ear);

        let from_right = parse(&message(0x2014, 0x02, 0x83, 0x0F, 0)).unwrap();
        assert!(!from_right.from_left);
        assert_eq!(from_right.right.battery, Some(30));
        assert!(from_right.right.in_ear);
        assert_eq!(from_right.left.battery, Some(80));
        assert!(!from_right.left.in_ear);
        assert!(!from_right.both_in_ear());
    }

    #[test]
    fn reads_the_case_and_charging() {
        // Both in the case, lid open, both earbuds and the case charging.
        let data = message(0x2013, 0x24, 0x77, 0x75, 0x02);
        let status = parse(&data).unwrap();
        assert!(status.both_in_case);
        assert_eq!(
            status.case,
            Case {
                battery: Some(50),
                charging: true
            }
        );
        assert!(status.left.charging && status.right.charging);
        assert_eq!(status.lid_open, Some(true));
    }

    #[test]
    fn only_the_lid_bit_decides_the_lid() {
        // Every other lid bit set, the counter included: still open.
        let open = parse(&message(0x2013, 0x24, 0x77, 0x05, 0xF7)).unwrap();
        assert_eq!(open.lid_open, Some(true));
        let closed = parse(&message(0x2013, 0x24, 0x77, 0x05, 0x08)).unwrap();
        assert_eq!(closed.lid_open, Some(false));
    }

    #[test]
    fn a_charging_earbud_is_not_in_an_ear() {
        // Both ear flags set, but this earbud charges in the case.
        let status = parse(&message(0x200F, 0x2A, 0x55, 0x15, 0)).unwrap();
        assert!(status.left.charging && !status.left.in_ear);
        assert!(!status.right.charging && status.right.in_ear);
        assert!(!status.both_in_ear());

        // The same from the right: the other earbud, now the left, charges.
        let status = parse(&message(0x200F, 0x0A, 0x55, 0x25, 0)).unwrap();
        assert!(status.left.charging && !status.left.in_ear);
        assert!(!status.right.charging && status.right.in_ear);
    }

    #[test]
    fn unknown_levels_are_none() {
        let status = parse(&message(0x200E, 0x20, 0xFB, 0x0F, 0)).unwrap();
        assert_eq!(status.left.battery, None, "11 is past the scale");
        assert_eq!(status.right.battery, None, "15 means not there");
        assert_eq!(status.case.battery, None);
    }

    #[test]
    fn full_is_a_hundred_percent() {
        let status = parse(&message(0x200E, 0x20, 0xA0, 0x0A, 0)).unwrap();
        assert_eq!(status.left.battery, Some(0));
        assert_eq!(status.right.battery, Some(100));
        assert_eq!(status.case.battery, Some(100));
    }

    #[test]
    fn finds_the_message_after_others() {
        let mut data = vec![0x10, 0x05, 1, 2, 3, 4, 5];
        data.extend(message(0x2024, 0x2B, 0x99, 0x8F, 0));
        let status = parse(&data).unwrap();
        assert_eq!(status.model, Model::AirPodsPro2UsbC);
        assert_eq!(status.model.name(), "AirPods Pro");
    }

    #[test]
    fn unknown_models_still_parse() {
        let status = parse(&message(0x2099, 0x2B, 0x99, 0x8F, 0)).unwrap();
        assert_eq!(status.model, Model::Other(0x2099));
        assert_eq!(status.model.name(), "AirPods");
    }

    #[test]
    fn other_apple_messages_are_ignored() {
        assert_eq!(parse(&[]), None);
        assert_eq!(
            parse(&[0x10, 0x05, 1, 2, 3, 4, 5]),
            None,
            "nearby info only"
        );
        assert_eq!(parse(&[0x07]), None, "a type with no length");

        // A proximity pairing message of another length comes from another
        // kind of device.
        let mut short = message(0x200E, 0x2B, 0x99, 0x8F, 0);
        short[1] = 0x11;
        short.truncate(2 + 0x11);
        assert_eq!(parse(&short), None);
    }

    #[test]
    fn truncated_data_is_refused() {
        let data = message(0x200E, 0x2B, 0x99, 0x8F, 0);
        for len in 0..data.len() {
            assert_eq!(parse(&data[..len]), None, "cut to {len} bytes");
        }
    }
}

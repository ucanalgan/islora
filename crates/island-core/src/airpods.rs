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
//!
//! [`Tracker`] sits on top of the parsed messages. It decides, from the
//! advertisements of one connected pair of AirPods, when the island should say
//! something: once on connecting, and once when a worn earbud runs low.

use crate::activity::Millis;
use crate::indicator::Indicator;

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

    /// The model id this was read from, so that the model id Windows reports
    /// for the connected headset can be compared with the one in a message.
    pub fn id(self) -> u16 {
        match self {
            Self::AirPods1 => 0x2002,
            Self::AirPods2 => 0x200F,
            Self::AirPods3 => 0x2013,
            Self::AirPods4 => 0x2019,
            Self::AirPodsPro => 0x200E,
            Self::AirPodsPro2 => 0x2014,
            Self::AirPodsPro2UsbC => 0x2024,
            Self::AirPodsPro3 => 0x2027,
            Self::AirPodsMax => 0x200A,
            Self::Other(id) => id,
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

/// The profiles of a headset's audio: A2DP sink and Hands-Free.
const AUDIO_PROFILES: [&str; 2] = ["{0000110B-", "{0000111E-"];
/// Apple's vendor id as Windows spells it in a Bluetooth device's instance id,
/// where the `0001` in front says that the Bluetooth SIG gave it out. The
/// product id follows `PID&`.
const APPLE_VENDOR: &str = "VID&0001004C_PID&";

/// The model id of the AirPods that these device instance ids belong to, if
/// they do. Windows lists a Bluetooth device's profiles as children, such as
/// `BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&2014\7&...`.
/// One must carry Apple's vendor id, and one must be an audio profile, which
/// keeps Apple's keyboards, mice and trackpads out, since they carry the same
/// vendor id.
///
/// Only the earbuds are known models here. AirPods Max have no earbuds or case
/// to read, and other Apple or Beats audio is not known to advertise the same
/// message, so they all give none and keep the generic notice.
pub fn apple_audio_pid(instance_ids: &[&str]) -> Option<u16> {
    let ids: Vec<String> = instance_ids.iter().map(|id| id.to_uppercase()).collect();
    if !ids
        .iter()
        .any(|id| AUDIO_PROFILES.iter().any(|profile| id.contains(profile)))
    {
        return None;
    }
    let pid = ids.iter().find_map(|id| {
        let (_, after) = id.split_once(APPLE_VENDOR)?;
        hex4(after.get(..4)?)
    })?;
    matches!(
        Model::from_id(pid),
        Model::AirPods1
            | Model::AirPods2
            | Model::AirPods3
            | Model::AirPods4
            | Model::AirPodsPro
            | Model::AirPodsPro2
            | Model::AirPodsPro2UsbC
            | Model::AirPodsPro3
    )
    .then_some(pid)
}

/// Exactly four hexadecimal digits, which `from_str_radix` alone would not
/// insist on: it also takes a leading plus sign.
fn hex4(text: &str) -> Option<u16> {
    if text.len() == 4 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        u16::from_str_radix(text, 16).ok()
    } else {
        None
    }
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

/// Advertisements weaker than this, in dBm, come from earbuds too far away to
/// be the ones in use. This is a guess to tune on a real device in step 2b.
///
/// It is the only guard against other people's AirPods of the same model
/// within range: what is advertised does not tell one pair from another, since
/// the part that would is encrypted.
const RSSI_MIN: i16 = -70;
/// How long an earbud's own report outranks what the other earbud says about
/// it. The other earbud's copy can lag behind, but it is better than a report
/// that is old.
const STALE_MS: Millis = 30_000;
/// How soon after connecting the first advertisement must come to still be
/// announced. A notice for a connection that is long past would only confuse.
const CONNECT_WINDOW_MS: Millis = 20_000;
/// Warns once on the way down past each of these percentages.
const LOW_WARNINGS: [u8; 2] = [20, 10];

/// The latest value read for one earbud.
#[derive(Clone, Copy, Debug)]
struct Reading {
    pod: Pod,
    /// Told by the earbud itself, not by the other one.
    first_hand: bool,
    at: Millis,
}

/// What is known about one earbud while connected.
#[derive(Clone, Copy, Debug)]
struct Slot {
    reading: Option<Reading>,
    /// Whether each of [`LOW_WARNINGS`] may still be given.
    armed: [bool; LOW_WARNINGS.len()],
}

impl Slot {
    fn new() -> Self {
        Self {
            reading: None,
            armed: [true; LOW_WARNINGS.len()],
        }
    }

    /// Takes in what a message says about this earbud. A level that is not
    /// known leaves what is held as it is, and so does a second-hand report
    /// while the earbud's own is recent.
    fn update(&mut self, pod: Pod, first_hand: bool, now: Millis) {
        if pod.battery.is_none() {
            return;
        }
        if !first_hand
            && let Some(held) = self.reading
            && held.first_hand
            && now.saturating_sub(held.at) <= STALE_MS
        {
            return;
        }
        self.reading = Some(Reading {
            pod,
            first_hand,
            at: now,
        });
        // Levels come in steps of ten, so a level that flickers around a
        // limit must not warn every time. Only seeing the earbud charge, which
        // is the one way its level really goes back up, warns again.
        if pod.charging {
            self.armed = [true; LOW_WARNINGS.len()];
        }
    }

    fn battery(&self) -> Option<u8> {
        self.reading.and_then(|reading| reading.pod.battery)
    }

    /// The level of this earbud if it has fallen to or past a warning level
    /// while being worn. Every level it has fallen past is used up, so that
    /// falling past both at once warns once.
    fn fallen(&mut self) -> Option<u8> {
        let pod = self.reading?.pod;
        let battery = pod.battery?;
        if !pod.in_ear || pod.charging {
            return None;
        }
        let mut fallen = false;
        for (armed, limit) in self.armed.iter_mut().zip(LOW_WARNINGS) {
            if *armed && battery <= limit {
                *armed = false;
                fallen = true;
            }
        }
        fallen.then_some(battery)
    }
}

/// One connection of AirPods, from connecting to disconnecting.
#[derive(Debug)]
struct Link {
    name: String,
    model_id: u16,
    connected_at: Millis,
    /// Left, then right.
    pods: [Slot; 2],
    case: Option<u8>,
    /// Whether an advertisement has been accepted yet, or the connect notice
    /// is not wanted.
    seen: bool,
}

/// Turns the advertisements of connected AirPods into what is worth saying.
/// It reads no clock and touches no Windows API: the caller says what time it
/// is, which headset Windows has connected, and what was advertised.
///
/// `now` must come from a monotonic clock. If it ever runs backwards nothing
/// panics, but ages stop growing, so staleness and the connect window freeze
/// until time catches up.
#[derive(Debug, Default)]
pub struct Tracker {
    link: Option<Link>,
}

impl Tracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// A paired headset with this model id has connected. Everything learned
    /// about an earlier connection is forgotten, warnings included, unless it
    /// is the same model that is already connected: Windows can report one
    /// connection twice, and that changes nothing. Surrounding whitespace is
    /// trimmed from the name, and an empty name is replaced by the model's own.
    pub fn connected(&mut self, name: String, model_id: u16, now: Millis) {
        self.connect(name, model_id, now, false);
    }

    /// Like [`Tracker::connected`], for a headset that was already connected
    /// when the app started. That is not news, so no connect notice comes, but
    /// the low battery warnings work.
    pub fn connected_quietly(&mut self, name: String, model_id: u16, now: Millis) {
        self.connect(name, model_id, now, true);
    }

    fn connect(&mut self, name: String, model_id: u16, now: Millis, quiet: bool) {
        if self
            .link
            .as_ref()
            .is_some_and(|link| link.model_id == model_id)
        {
            return;
        }
        let name = name.trim();
        let name = if name.is_empty() {
            Model::from_id(model_id).name().to_owned()
        } else {
            name.to_owned()
        };
        self.link = Some(Link {
            name,
            model_id,
            connected_at: now,
            pods: [Slot::new(); 2],
            case: None,
            seen: quiet,
        });
    }

    /// Gives up on announcing a connection that has been waited for in vain,
    /// say because no advertisement came, so that the caller can say it
    /// another way. Returns whether the connect notice was still owed. In
    /// that case it is used up, so no connect notice comes from here later,
    /// and the caller must say it. After false, someone else has: the notice
    /// was given, or was not wanted, or this model is not connected.
    ///
    /// Warnings are not affected either way.
    pub fn give_up_announcing(&mut self, model_id: u16) -> bool {
        match self.link.as_mut() {
            Some(link) if link.model_id == model_id && !link.seen => {
                link.seen = true;
                true
            }
            _ => false,
        }
    }

    /// The headset with this model id has disconnected. A report about some
    /// other model than the connected one is not about this connection and is
    /// ignored.
    pub fn disconnected(&mut self, model_id: u16) {
        if self
            .link
            .as_ref()
            .is_some_and(|link| link.model_id == model_id)
        {
            self.link = None;
        }
    }

    /// An advertisement came in. Ones from another model, from too far away,
    /// or while nothing is connected are not about the headset in use.
    ///
    /// The first accepted one soon after connecting returns the connect
    /// notice. After that, an earbud that is worn and not charging returns a
    /// low battery notice each time it falls past a warning level. Only one
    /// notice comes back per call.
    pub fn advertised(&mut self, status: &Status, rssi: i16, now: Millis) -> Option<Indicator> {
        let link = self.link.as_mut()?;
        if status.model.id() != link.model_id || rssi < RSSI_MIN {
            return None;
        }
        let sides = [
            (status.left, status.from_left),
            (status.right, !status.from_left),
        ];
        for (slot, (pod, first_hand)) in link.pods.iter_mut().zip(sides) {
            slot.update(pod, first_hand, now);
        }
        if let Some(case) = status.case.battery {
            link.case = Some(case);
        }

        // Levels that the connect notice shows are used up here too, so that
        // no warning follows right after it.
        let [left, right] = link.pods.each_mut().map(Slot::fallen);
        let first = !link.seen;
        link.seen = true;
        if first && now.saturating_sub(link.connected_at) <= CONNECT_WINDOW_MS {
            return Some(Indicator::AirPods {
                name: link.name.clone(),
                left: link.pods[0].battery(),
                right: link.pods[1].battery(),
                case: link.case,
            });
        }
        (left.is_some() || right.is_some()).then_some(Indicator::AirPodsLow { left, right })
    }
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

    #[test]
    fn a_model_gives_back_the_id_it_was_read_from() {
        for id in 0..=u16::MAX {
            assert_eq!(Model::from_id(id).id(), id);
        }
        assert_eq!(Model::AirPodsPro2.id(), 0x2014);
        assert_eq!(Model::Other(0x2099).id(), 0x2099);
    }

    const PRO: u16 = 0x200E;
    const NAME: &str = "Umut'un AirPods";

    /// An earbud in an ear, not charging.
    fn worn(battery: u8) -> Pod {
        Pod {
            battery: Some(battery),
            charging: false,
            in_ear: true,
        }
    }

    fn pod(battery: Option<u8>, charging: bool, in_ear: bool) -> Pod {
        Pod {
            battery,
            charging,
            in_ear,
        }
    }

    fn status_of(model: u16, from_left: bool, left: Pod, right: Pod, case: Option<u8>) -> Status {
        Status {
            model: Model::from_id(model),
            left,
            right,
            case: Case {
                battery: case,
                charging: false,
            },
            both_in_case: false,
            from_left,
            lid_open: None,
        }
    }

    /// An advertisement from the left earbud, which is first-hand for it.
    fn from_left(left: Pod, right: Pod) -> Status {
        status_of(PRO, true, left, right, None)
    }

    /// An advertisement from the right earbud.
    fn from_right(left: Pod, right: Pod) -> Status {
        status_of(PRO, false, left, right, None)
    }

    const NEAR: i16 = -50;

    /// A tracker that is connected and has already given its connect notice.
    fn announced() -> Tracker {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 0);
        let first = tracker.advertised(&from_left(worn(90), worn(90)), NEAR, 1_000);
        assert!(matches!(first, Some(Indicator::AirPods { .. })));
        tracker
    }

    fn low(left: Option<u8>, right: Option<u8>) -> Option<Indicator> {
        Some(Indicator::AirPodsLow { left, right })
    }

    #[test]
    fn nothing_is_said_while_nothing_is_connected() {
        let mut tracker = Tracker::new();
        assert_eq!(
            tracker.advertised(&from_left(worn(90), worn(90)), NEAR, 0),
            None
        );

        tracker.connected(NAME.into(), PRO, 0);
        tracker.disconnected(PRO);
        assert_eq!(
            tracker.advertised(&from_left(worn(90), worn(90)), NEAR, 1_000),
            None
        );
    }

    #[test]
    fn another_model_is_ignored() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 0);
        let other = status_of(0x2014, true, worn(90), worn(90), None);
        assert_eq!(tracker.advertised(&other, NEAR, 1_000), None);
        // The refused one did not use up the connect notice.
        assert!(
            tracker
                .advertised(&from_left(worn(90), worn(90)), NEAR, 2_000)
                .is_some()
        );
    }

    #[test]
    fn weak_signals_are_ignored() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 0);
        let status = from_left(worn(90), worn(90));
        assert_eq!(tracker.advertised(&status, RSSI_MIN - 1, 1_000), None);
        assert!(tracker.advertised(&status, RSSI_MIN, 2_000).is_some());
    }

    #[test]
    fn a_weak_signal_does_not_change_what_is_known() {
        let mut tracker = announced();
        // A stranger's earbuds at 10 %, heard faintly, warn about nothing and
        // use up no warning.
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(10)), -90, 2_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 3_000),
            low(Some(20), None)
        );
    }

    #[test]
    fn connecting_announces_the_levels_once() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 5_000);
        let status = status_of(PRO, true, worn(80), worn(90), Some(50));
        assert_eq!(
            tracker.advertised(&status, NEAR, 6_000),
            Some(Indicator::AirPods {
                name: NAME.into(),
                left: Some(80),
                right: Some(90),
                case: Some(50),
            })
        );
        assert_eq!(tracker.advertised(&status, NEAR, 7_000), None);
    }

    #[test]
    fn the_connect_notice_has_a_window() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 5_000);
        let status = from_left(worn(80), worn(90));
        let edge = 5_000 + CONNECT_WINDOW_MS;
        assert!(tracker.advertised(&status, NEAR, edge).is_some());

        let mut late = Tracker::new();
        late.connected(NAME.into(), PRO, 5_000);
        assert_eq!(late.advertised(&status, NEAR, edge + 1), None);
        // Having been too late once, a later one is not on time either.
        assert_eq!(late.advertised(&status, NEAR, edge + 2), None);
    }

    #[test]
    fn a_late_first_advertisement_still_warns_about_a_low_earbud() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 0);
        let status = from_left(worn(20), worn(90));
        assert_eq!(
            tracker.advertised(&status, NEAR, CONNECT_WINDOW_MS + 1),
            low(Some(20), None)
        );
    }

    #[test]
    fn an_empty_name_becomes_the_models() {
        let mut tracker = Tracker::new();
        tracker.connected("  ".into(), 0x2014, 0);
        let status = status_of(0x2014, true, worn(80), worn(80), None);
        let Some(Indicator::AirPods { name, .. }) = tracker.advertised(&status, NEAR, 1_000) else {
            panic!("no connect notice");
        };
        assert_eq!(name, "AirPods Pro");
    }

    #[test]
    fn the_name_is_trimmed() {
        let mut tracker = Tracker::new();
        tracker.connected("  Umut'un AirPods \t".into(), PRO, 0);
        let status = from_left(worn(80), worn(80));
        let Some(Indicator::AirPods { name, .. }) = tracker.advertised(&status, NEAR, 1_000) else {
            panic!("no connect notice");
        };
        assert_eq!(name, NAME);
    }

    #[test]
    fn unknown_levels_are_left_out_of_the_connect_notice() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 0);
        // The right earbud is not there, so it says nothing, and neither does
        // the case.
        let status = from_left(worn(70), pod(None, false, false));
        assert_eq!(
            tracker.advertised(&status, NEAR, 1_000),
            Some(Indicator::AirPods {
                name: NAME.into(),
                left: Some(70),
                right: None,
                case: None,
            })
        );
    }

    #[test]
    fn a_new_connection_forgets_the_case_level() {
        let mut tracker = announced();
        let with_case = status_of(PRO, true, worn(90), worn(90), Some(40));
        tracker.advertised(&with_case, NEAR, 2_000);
        tracker.disconnected(PRO);
        tracker.connected(NAME.into(), PRO, 10_000);
        let status = from_left(worn(90), worn(90));
        assert_eq!(
            tracker.advertised(&status, NEAR, 11_000),
            Some(Indicator::AirPods {
                name: NAME.into(),
                left: Some(90),
                right: Some(90),
                case: None,
            })
        );
    }

    #[test]
    fn an_earbuds_own_report_wins_over_the_other_ones() {
        let mut tracker = announced();
        // The left earbud has said it is at 90 %. A moment later the right
        // one passes on 20 % for it, which must not count: taken in, it
        // would warn at once.
        assert_eq!(
            tracker.advertised(&from_right(worn(20), worn(90)), NEAR, 2_000),
            None
        );
        // The left earbud's own report of 20 % does.
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 3_000),
            low(Some(20), None)
        );
    }

    #[test]
    fn a_second_hand_report_counts_once_the_first_hand_one_is_stale() {
        let mut tracker = announced();
        // The left earbud last spoke for itself at 1 s.
        let later = 1_000 + STALE_MS + 1;
        assert_eq!(
            tracker.advertised(&from_right(worn(20), worn(90)), NEAR, later),
            low(Some(20), None)
        );
    }

    #[test]
    fn a_second_hand_report_is_ignored_while_the_first_hand_one_is_recent() {
        let mut tracker = announced();
        // Exactly at the window's edge the left earbud's own report is still
        // recent.
        let edge = 1_000 + STALE_MS;
        assert_eq!(
            tracker.advertised(&from_right(worn(20), worn(90)), NEAR, edge),
            None
        );
    }

    #[test]
    fn a_second_hand_report_counts_when_there_is_no_other() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 0);
        // Only the right earbud has spoken, so the left earbud's level is
        // known second-hand.
        let status = from_right(worn(60), worn(80));
        assert_eq!(
            tracker.advertised(&status, NEAR, 1_000),
            Some(Indicator::AirPods {
                name: NAME.into(),
                left: Some(60),
                right: Some(80),
                case: None,
            })
        );
    }

    #[test]
    fn each_level_warns_once() {
        let mut tracker = announced();
        assert_eq!(
            tracker.advertised(&from_left(worn(30), worn(90)), NEAR, 2_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 3_000),
            low(Some(20), None)
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 4_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(90)), NEAR, 5_000),
            low(Some(10), None)
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(90)), NEAR, 6_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(0), worn(90)), NEAR, 7_000),
            None
        );
    }

    #[test]
    fn falling_past_both_levels_at_once_warns_once() {
        let mut tracker = announced();
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(90)), NEAR, 2_000),
            low(Some(10), None)
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(90)), NEAR, 3_000),
            None
        );
    }

    #[test]
    fn the_right_earbud_warns_on_its_own() {
        let mut tracker = announced();
        assert_eq!(
            tracker.advertised(&from_right(worn(90), worn(20)), NEAR, 2_000),
            low(None, Some(20))
        );
    }

    #[test]
    fn both_earbuds_falling_together_come_as_one_notice() {
        let mut tracker = announced();
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(20)), NEAR, 2_000),
            low(Some(20), Some(20))
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(20)), NEAR, 3_000),
            None
        );
        // Each level is then warned about on its own again.
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(20)), NEAR, 4_000),
            low(Some(10), None)
        );
    }

    #[test]
    fn earbuds_that_are_not_worn_are_not_warned_about() {
        let mut tracker = announced();
        let in_case = pod(Some(10), true, false);
        let in_hand = pod(Some(10), false, false);
        assert_eq!(
            tracker.advertised(&from_left(in_case, in_hand), NEAR, 2_000),
            None
        );
        // Putting one on afterwards is the first time it is worn low.
        assert_eq!(
            tracker.advertised(&from_left(worn(10), in_hand), NEAR, 3_000),
            low(Some(10), None)
        );
    }

    #[test]
    fn an_earbud_is_warned_again_only_after_it_has_charged() {
        let mut tracker = announced();
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 2_000),
            low(Some(20), None)
        );
        // Staying at the level warns no more.
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 3_000),
            None
        );
        // A level that flickers up and down warns only once.
        assert_eq!(
            tracker.advertised(&from_left(worn(30), worn(90)), NEAR, 4_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 5_000),
            None
        );
        // Seeing it charge in the case is what warns again.
        let charging = pod(Some(30), true, false);
        assert_eq!(
            tracker.advertised(&from_left(charging, worn(90)), NEAR, 6_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 7_000),
            low(Some(20), None)
        );
    }

    #[test]
    fn charging_rearms_every_level_of_that_earbud_only() {
        let mut tracker = announced();
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(10)), NEAR, 2_000),
            low(Some(10), Some(10))
        );
        // Only the left earbud charges.
        let charging = pod(Some(40), true, false);
        assert_eq!(
            tracker.advertised(&from_left(charging, worn(10)), NEAR, 3_000),
            None
        );
        // The left earbud warns at both levels again, the right one not.
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(10)), NEAR, 4_000),
            low(Some(20), None)
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(10)), NEAR, 5_000),
            low(Some(10), None)
        );
    }

    #[test]
    fn a_second_hand_charging_report_is_not_taken_while_the_own_one_is_recent() {
        let mut tracker = announced();
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 2_000),
            low(Some(20), None)
        );
        // The right earbud says the left one charges, but the left one's own
        // report from a moment ago says it is worn.
        let charging = pod(Some(30), true, false);
        assert_eq!(
            tracker.advertised(&from_right(charging, worn(90)), NEAR, 3_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 4_000),
            None
        );
    }

    #[test]
    fn unknown_levels_warn_about_nothing_and_change_nothing() {
        let mut tracker = announced();
        let unknown = pod(None, false, true);
        assert_eq!(
            tracker.advertised(&from_left(unknown, worn(90)), NEAR, 2_000),
            None
        );
        // The level held from before is still 90 %, so 20 % is a fall.
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 3_000),
            low(Some(20), None)
        );

        // An unknown level that comes with the charging flag does not warn
        // about the used up levels again either.
        let unknown_charging = pod(None, true, false);
        assert_eq!(
            tracker.advertised(&from_left(unknown_charging, worn(90)), NEAR, 4_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 5_000),
            None
        );
    }

    #[test]
    fn an_unknown_level_does_not_make_the_earbuds_own_report_recent() {
        let mut tracker = announced();
        // The left earbud last gave a level at 1 s. Its unknown level at 25 s
        // says nothing, so at 35 s that report is stale and the right
        // earbud's 20 % for it counts.
        let unknown = pod(None, false, true);
        assert_eq!(
            tracker.advertised(&from_left(unknown, worn(90)), NEAR, 25_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_right(worn(20), worn(90)), NEAR, 35_000),
            low(Some(20), None)
        );
    }

    #[test]
    fn no_warning_follows_the_connect_notice_for_the_levels_it_shows() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 0);
        let first = tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 1_000);
        assert!(matches!(first, Some(Indicator::AirPods { .. })));
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 2_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(90)), NEAR, 3_000),
            low(Some(10), None)
        );
    }

    #[test]
    fn a_new_connection_starts_over() {
        let mut tracker = announced();
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 2_000),
            low(Some(20), None)
        );

        tracker.disconnected(PRO);
        tracker.connected(NAME.into(), PRO, 100_000);
        let status = from_left(worn(90), worn(90));
        assert!(matches!(
            tracker.advertised(&status, NEAR, 101_000),
            Some(Indicator::AirPods { .. })
        ));
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 102_000),
            low(Some(20), None)
        );
    }

    #[test]
    fn connecting_again_changes_nothing() {
        let mut tracker = announced();
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 2_000),
            low(Some(20), None)
        );

        // Windows reports the same connection a second time, late enough that
        // a new connection would be announced.
        tracker.connected("Another name".into(), PRO, 100_000);
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 101_000),
            None,
            "neither announced again nor warned again"
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(90)), NEAR, 102_000),
            low(Some(10), None)
        );
    }

    #[test]
    fn connecting_again_keeps_the_connect_window() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 0);
        tracker.connected(NAME.into(), PRO, CONNECT_WINDOW_MS);
        // Measured from the first report, so this is too late.
        let status = from_left(worn(80), worn(80));
        assert_eq!(
            tracker.advertised(&status, NEAR, CONNECT_WINDOW_MS + 1),
            None
        );
    }

    #[test]
    fn a_different_model_replaces_the_connection() {
        let mut tracker = announced();
        tracker.connected("AirPods Max".into(), 0x200A, 10_000);
        // The earlier one's advertisements are not about it any more.
        assert_eq!(
            tracker.advertised(&from_left(worn(80), worn(80)), NEAR, 11_000),
            None
        );
        let max = status_of(0x200A, true, worn(80), worn(80), None);
        assert!(matches!(
            tracker.advertised(&max, NEAR, 12_000),
            Some(Indicator::AirPods { .. })
        ));
    }

    #[test]
    fn disconnecting_another_model_is_ignored() {
        let mut tracker = announced();
        tracker.disconnected(0x200A);
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 2_000),
            low(Some(20), None)
        );
        tracker.disconnected(PRO);
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(90)), NEAR, 3_000),
            None
        );
    }

    #[test]
    fn a_quiet_connection_has_no_connect_notice_but_warns() {
        let mut tracker = Tracker::new();
        tracker.connected_quietly(NAME.into(), PRO, 0);
        assert_eq!(
            tracker.advertised(&from_left(worn(90), worn(90)), NEAR, 1_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 2_000),
            low(Some(20), None)
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 3_000),
            None
        );
    }

    #[test]
    fn a_quiet_connection_warns_about_what_is_already_low() {
        let mut tracker = Tracker::new();
        tracker.connected_quietly(NAME.into(), PRO, 0);
        assert_eq!(
            tracker.advertised(&from_left(worn(10), worn(90)), NEAR, 1_000),
            low(Some(10), None)
        );
    }

    #[test]
    fn connecting_for_real_after_a_quiet_one_changes_nothing() {
        let mut tracker = Tracker::new();
        tracker.connected_quietly(NAME.into(), PRO, 0);
        tracker.connected(NAME.into(), PRO, 1_000);
        assert_eq!(
            tracker.advertised(&from_left(worn(90), worn(90)), NEAR, 2_000),
            None
        );
    }

    /// A Bluetooth device's profile children as Windows lists them.
    fn profile(uuid: &str, pid: &str) -> String {
        format!(
            "BTHENUM\\{{{uuid}-0000-1000-8000-00805f9b34fb}}_VID&0001004c_PID&{pid}\\7&2a1b3c4d&0&A1B2C3D4E5F6_C00000000"
        )
    }

    #[test]
    fn airpods_are_found_by_their_vendor_and_audio_profile() {
        let sink = profile("0000110b", "2014");
        assert_eq!(apple_audio_pid(&[&sink]), Some(0x2014));
        let hands_free = profile("0000111e", "2027");
        assert_eq!(apple_audio_pid(&[&hands_free]), Some(0x2027));
    }

    #[test]
    fn the_vendor_and_the_audio_profile_may_be_on_different_ids() {
        let device = "BTHENUM\\DEV_A1B2C3D4E5F6\\7&2a1b3c4d&0&BLUETOOTHDEVICE_A1B2C3D4E5F6";
        let audio =
            "BTHENUM\\{0000110B-0000-1000-8000-00805F9B34FB}\\7&2A1B3C4D&0&A1B2C3D4E5F6_C00000000";
        let vendor = profile("00001200", "200E");
        assert_eq!(apple_audio_pid(&[device, audio, &vendor]), Some(0x200E));
    }

    #[test]
    fn ids_are_read_whatever_their_case() {
        let id = "bthenum\\{0000111E-0000-1000-8000-00805f9b34fb}_vid&0001004c_pid&200f\\x";
        assert_eq!(apple_audio_pid(&[id]), Some(0x200F));
    }

    #[test]
    fn apple_devices_without_audio_are_not_airpods() {
        // A keyboard has Apple's vendor id and a human interface profile.
        let keyboard = profile("00001124", "0267");
        assert_eq!(apple_audio_pid(&[&keyboard]), None);
        assert_eq!(apple_audio_pid(&[]), None);
    }

    #[test]
    fn other_vendors_audio_is_not_airpods() {
        let sony = "BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001012d_PID&0e3d\\7&1&0&001122334455_C00000000";
        assert_eq!(apple_audio_pid(&[sony]), None);
        // The Bluetooth SIG source is the one that counts: USB's source id
        // for the same number is another company's.
        let usb_source = profile("0000110b", "2014").replace("VID&0001004c", "VID&0002004c");
        assert_eq!(apple_audio_pid(&[&usb_source]), None);
    }

    #[test]
    fn only_known_earbuds_are_taken() {
        let max = profile("0000110b", "200a");
        assert_eq!(apple_audio_pid(&[&max]), None, "AirPods Max");
        // A Beats product, or an Apple one that is not known, is not.
        let other = profile("0000110b", "2006");
        assert_eq!(apple_audio_pid(&[&other]), None, "another model");
        for known in [
            "2002", "200f", "2013", "2019", "200e", "2014", "2024", "2027",
        ] {
            let id = profile("0000110b", known);
            assert!(apple_audio_pid(&[&id]).is_some(), "pid {known}");
        }
    }

    #[test]
    fn a_product_id_is_four_hex_digits_and_nothing_else() {
        assert_eq!(hex4("2027"), Some(0x2027));
        assert_eq!(hex4("20aF"), Some(0x20AF));
        for text in ["+201", "-201", "201", "20277", " 201", "20 7", "2g27", ""] {
            assert_eq!(hex4(text), None, "{text:?}");
        }
    }

    #[test]
    fn giving_up_announcing_is_owed_once() {
        let mut tracker = Tracker::new();
        assert!(!tracker.give_up_announcing(PRO), "nothing connected");
        tracker.connected(NAME.into(), PRO, 0);
        assert!(!tracker.give_up_announcing(0x2014), "another model");
        assert!(tracker.give_up_announcing(PRO));
        assert!(!tracker.give_up_announcing(PRO), "already given up");
        // Having given up, the connect notice does not come any more, but
        // warnings do.
        assert_eq!(
            tracker.advertised(&from_left(worn(90), worn(90)), NEAR, 1_000),
            None
        );
        assert_eq!(
            tracker.advertised(&from_left(worn(20), worn(90)), NEAR, 2_000),
            low(Some(20), None)
        );
    }

    #[test]
    fn nothing_is_owed_once_the_connection_was_announced() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 0);
        let first = tracker.advertised(&from_left(worn(90), worn(90)), NEAR, 1_000);
        assert!(matches!(first, Some(Indicator::AirPods { .. })));
        assert!(!tracker.give_up_announcing(PRO));
    }

    #[test]
    fn a_quiet_connection_owes_nothing() {
        let mut tracker = Tracker::new();
        tracker.connected_quietly(NAME.into(), PRO, 0);
        assert!(!tracker.give_up_announcing(PRO));
    }

    #[test]
    fn a_new_connection_owes_its_notice_again() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 0);
        assert!(tracker.give_up_announcing(PRO));
        tracker.disconnected(PRO);
        tracker.connected(NAME.into(), PRO, 100_000);
        assert!(tracker.give_up_announcing(PRO));
    }

    #[test]
    fn a_broken_product_id_gives_nothing() {
        for pid in ["", "20", "ZZZZ", "20-4", "+201"] {
            let id = profile("0000110b", pid);
            assert_eq!(apple_audio_pid(&[&id]), None, "pid {pid:?}");
        }
    }

    #[test]
    fn a_clock_that_runs_backwards_does_not_panic() {
        let mut tracker = Tracker::new();
        tracker.connected(NAME.into(), PRO, 10_000);
        let status = from_left(worn(80), worn(80));
        assert!(tracker.advertised(&status, NEAR, 0).is_some());
        assert_eq!(tracker.advertised(&status, NEAR, 0), None);
    }
}

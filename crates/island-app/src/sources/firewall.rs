//! Reads Windows' firewall for what it does with this app on a network, away
//! from the island's thread, as going through its rules takes a moment. It
//! only reads: Islora never changes the firewall.

use island_core::firewall::{self, Profile, Reach, Rule};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, NET_FW_ACTION_ALLOW, NET_FW_PROFILE_TYPE2, NET_FW_RULE_DIR_IN, NetFwPolicy2,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, IDispatch};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH, VariantClear};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{Interface, Result};

pub const MSG_FIREWALL: u32 = WM_APP + 23;

/// Looks on a thread of its own and posts the answer to `window`, with
/// `ticket` to tell it apart from the answer for an earlier send.
pub fn check(window: HWND, profile: u32, ticket: usize) {
    let window = window.0 as isize;
    let looked = std::thread::Builder::new().name("firewall".into()).spawn(move || {
        // A firewall that cannot be read is no reason to worry the user.
        let reach = read(profile).unwrap_or(Reach::Open);
        let code = match reach {
            Reach::Open => 0,
            Reach::Asks => 1,
            Reach::Blocked => 2,
        };
        let _ = unsafe { PostMessageW(Some(HWND(window as _)), MSG_FIREWALL, WPARAM(code), LPARAM(ticket as isize)) };
    });
    if looked.is_err() {
        eprintln!("firewall thread failed to start");
    }
}

/// Decodes `MSG_FIREWALL`: the answer and the ticket it was asked with.
pub fn answer(wparam: WPARAM, lparam: LPARAM) -> (Reach, usize) {
    let reach = match wparam.0 {
        1 => Reach::Asks,
        2 => Reach::Blocked,
        _ => Reach::Open,
    };
    (reach, lparam.0 as usize)
}

fn read(profile: u32) -> Result<Reach> {
    let exe = std::env::current_exe().map(|path| path.to_string_lossy().to_lowercase()).unwrap_or_default();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_ALL)?;
        let kind = NET_FW_PROFILE_TYPE2(profile as i32);
        let settings = Profile {
            enabled: policy.get_FirewallEnabled(kind)?.as_bool(),
            block_all: policy.get_BlockAllInboundTraffic(kind)?.as_bool(),
            default_allow: policy.get_DefaultInboundAction(kind)? == NET_FW_ACTION_ALLOW,
        };
        let enumerator: IEnumVARIANT = policy.Rules()?._NewEnum()?.cast()?;
        let mut mine = Vec::new();
        loop {
            let mut item = [VARIANT::default()];
            let mut fetched = 0;
            if enumerator.Next(&mut item, &mut fetched).is_err() || fetched == 0 {
                break;
            }
            if let Some(rule) = rule_for(&item[0], &exe) {
                mine.push(rule);
            }
            let _ = VariantClear(&mut item[0]);
        }
        Ok(firewall::reach(settings, &mine, profile))
    }
}

/// The rule in `item`, if it names the program at `exe`.
fn rule_for(item: &VARIANT, exe: &str) -> Option<Rule> {
    unsafe {
        let inner = &item.Anonymous.Anonymous;
        if inner.vt != VT_DISPATCH {
            return None;
        }
        let dispatch: &IDispatch = inner.Anonymous.pdispVal.as_ref()?;
        let rule: INetFwRule = dispatch.cast().ok()?;
        if rule.ApplicationName().ok()?.to_string().to_lowercase() != exe {
            return None;
        }
        Some(Rule {
            inbound: rule.Direction().ok()? == NET_FW_RULE_DIR_IN,
            allow: rule.Action().ok()? == NET_FW_ACTION_ALLOW,
            enabled: rule.Enabled().ok()?.as_bool(),
            profiles: rule.Profiles().ok()? as u32,
            protocol: rule.Protocol().ok()?,
        })
    }
}

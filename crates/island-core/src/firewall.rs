//! Whether Windows' firewall lets a phone reach the island's small web server,
//! worked out from the firewall's own settings and its rules for this app.
//! Reading them is the app's job; this only decides.

/// The firewall's profiles, as Windows numbers them.
pub const DOMAIN: u32 = 1;
pub const PRIVATE: u32 = 2;
pub const PUBLIC: u32 = 4;

const TCP: i32 = 6;
const ANY_PROTOCOL: i32 = 256;

/// The firewall's settings for one profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub enabled: bool,
    /// Blocks every connection coming in, whatever the rules say.
    pub block_all: bool,
    /// Lets in what no rule speaks of.
    pub default_allow: bool,
}

/// One rule naming this app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub inbound: bool,
    pub allow: bool,
    pub enabled: bool,
    /// The profiles it applies to, as a set of the bits above.
    pub profiles: u32,
    pub protocol: i32,
}

impl Rule {
    fn speaks_of_tcp(&self) -> bool {
        self.enabled && self.inbound && (self.protocol == TCP || self.protocol == ANY_PROTOCOL)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Nothing stands in the way.
    Open,
    /// No rule yet, so Windows asks the first time the app listens.
    Asks,
    /// The firewall turns phones away on this network.
    Blocked,
}

impl Reach {
    /// What the open code says about it, if anything.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Reach::Open => None,
            Reach::Asks => Some("Windows izin sorarsa izin ver"),
            Reach::Blocked => Some("Güvenlik duvarı bu ağda engelliyor"),
        }
    }
}

/// Whether a phone gets through on a network of `profile`, given the firewall's
/// settings for it and every rule naming this app. A rule that blocks wins over
/// one that allows, as in Windows.
pub fn reach(settings: Profile, rules: &[Rule], profile: u32) -> Reach {
    if !settings.enabled {
        return Reach::Open;
    }
    if settings.block_all {
        return Reach::Blocked;
    }
    let mine: Vec<&Rule> = rules.iter().filter(|rule| rule.speaks_of_tcp()).collect();
    let here: Vec<&&Rule> = mine
        .iter()
        .filter(|rule| rule.profiles & profile != 0)
        .collect();
    if here.iter().any(|rule| !rule.allow) {
        return Reach::Blocked;
    }
    if here.iter().any(|rule| rule.allow) || settings.default_allow {
        return Reach::Open;
    }
    // Rules for other networks only: the user allowed it elsewhere, not here,
    // and Windows does not ask again.
    if mine.is_empty() {
        Reach::Asks
    } else {
        Reach::Blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: Profile = Profile {
        enabled: true,
        block_all: false,
        default_allow: false,
    };

    fn rule(allow: bool, profiles: u32) -> Rule {
        Rule {
            inbound: true,
            allow,
            enabled: true,
            profiles,
            protocol: TCP,
        }
    }

    #[test]
    fn allowed_on_this_network_gets_through() {
        assert_eq!(reach(ON, &[rule(true, PUBLIC)], PUBLIC), Reach::Open);
        assert_eq!(
            reach(ON, &[rule(true, PRIVATE | PUBLIC)], PRIVATE),
            Reach::Open
        );
    }

    #[test]
    fn allowed_only_on_other_networks_is_blocked_here() {
        assert_eq!(reach(ON, &[rule(true, PRIVATE)], PUBLIC), Reach::Blocked);
    }

    #[test]
    fn a_blocking_rule_wins() {
        assert_eq!(
            reach(ON, &[rule(true, PUBLIC), rule(false, PUBLIC)], PUBLIC),
            Reach::Blocked
        );
    }

    #[test]
    fn without_rules_windows_asks() {
        assert_eq!(reach(ON, &[], PUBLIC), Reach::Asks);
    }

    #[test]
    fn rules_that_do_not_apply_are_left_out() {
        let udp = Rule {
            protocol: 17,
            ..rule(false, PUBLIC)
        };
        let off = Rule {
            enabled: false,
            ..rule(false, PUBLIC)
        };
        let outbound = Rule {
            inbound: false,
            ..rule(false, PUBLIC)
        };
        assert_eq!(
            reach(ON, &[udp, off, outbound, rule(true, PUBLIC)], PUBLIC),
            Reach::Open
        );
        let any = Rule {
            protocol: ANY_PROTOCOL,
            ..rule(true, PUBLIC)
        };
        assert_eq!(reach(ON, &[any], PUBLIC), Reach::Open);
    }

    #[test]
    fn the_profile_settings_come_first() {
        let off = Profile {
            enabled: false,
            ..ON
        };
        assert_eq!(reach(off, &[rule(false, PUBLIC)], PUBLIC), Reach::Open);
        let shields_up = Profile {
            block_all: true,
            ..ON
        };
        assert_eq!(
            reach(shields_up, &[rule(true, PUBLIC)], PUBLIC),
            Reach::Blocked
        );
        let open_door = Profile {
            default_allow: true,
            ..ON
        };
        assert_eq!(reach(open_door, &[], PUBLIC), Reach::Open);
    }

    #[test]
    fn only_trouble_is_worth_a_word() {
        assert_eq!(Reach::Open.warning(), None);
        assert!(Reach::Asks.warning().is_some());
        assert!(Reach::Blocked.warning().is_some());
    }
}

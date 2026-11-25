//! Pausing music when an earbud comes out, and starting it again when it goes
//! back in, but only if this was what paused it.
//!
//! [`EarControl`] is driven from the UI thread: it is told how many earbuds are
//! in an ear, and what the followed player does, and says when to pause or
//! resume. It reads no clock and touches no player.
//!
//! The worst it can do is start music nobody asked for, so it only resumes what
//! it is sure it paused itself: the player must have been seen to stop after
//! the pause, nothing may have taken over since, and it must be the very
//! session that it paused.

use crate::activity::Millis;

/// A change in how many earbuds are worn must hold this long, as told by two
/// reports that far apart, to count. The earbuds report several times a
/// second while something changes, and a moment of flicker, such as a bud
/// being adjusted in the ear, is not worth stopping the music for. This is a
/// first guess, to tune on a real device.
pub const SETTLE_MS: Millis = 700;
/// Music that was paused by an earbud coming out is started again only if it
/// goes back in within this long. Putting the earbuds on hours later should
/// not start music.
pub const RESUME_WITHIN_MS: Millis = 10 * 60 * 1000;
/// How long a player may take to show that it stopped after a pause. If it is
/// still playing after that, or nothing was heard, the pause failed, and
/// whoever pauses it later is the user.
const PAUSE_SETTLE_MS: Millis = 2_000;

/// What to do to the player, which is named by the session that it was paused
/// in, so that it can never be done to another app's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EarAction {
    Pause(u64),
    Resume(u64),
}

/// What this has paused, and what it takes to start it again.
#[derive(Clone, Copy, Debug)]
struct Paused {
    /// How many earbuds were worn before they came out. It has to be that many
    /// again.
    before: u8,
    at: Millis,
    session: u64,
    /// Whether the player was seen to stop after the pause. Until then, it
    /// playing is only a slow answer; after, it playing is someone starting it.
    stop_seen: bool,
}

#[derive(Debug, Default)]
pub struct EarControl {
    /// The count that has held long enough. `None` until the first report,
    /// which only says where things stand.
    confirmed: Option<u8>,
    /// A different count that has been reported, and since when.
    candidate: Option<(u8, Millis)>,
    playing: bool,
    /// Whether the sound goes to the AirPods. If it does not, taking an earbud
    /// out has nothing to do with what is heard, so nothing is paused or
    /// resumed. Not knowing counts as no.
    output_airpods: bool,
    /// The session of the followed player's latest report, if it had one.
    session: Option<u64>,
    paused: Option<Paused>,
}

impl EarControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many earbuds are in an ear now, as the earbuds say. `None` is
    /// nothing known, which changes nothing: to forget everything, use
    /// [`EarControl::disconnected`].
    ///
    /// Call this for every report, also when the count is the same: the next
    /// report is what confirms a change.
    pub fn worn(&mut self, count: Option<u8>, now: Millis) -> Option<EarAction> {
        self.expire(now);
        let count = count?;
        let Some(confirmed) = self.confirmed else {
            self.confirmed = Some(count);
            return None;
        };
        if count == confirmed {
            self.candidate = None;
            return self.resume();
        }
        match self.candidate {
            Some((candidate, since)) if candidate == count => {
                if now.saturating_sub(since) < SETTLE_MS {
                    return None;
                }
            }
            _ => {
                self.candidate = Some((count, now));
                return None;
            }
        }
        self.candidate = None;
        self.confirmed = Some(count);
        if count < confirmed {
            return self.pause(confirmed, now);
        }
        self.resume()
    }

    /// What the followed player does, as of now: whether it plays, and in
    /// which session. A report without a session, as when nothing plays or
    /// the player stopped, says that nothing plays, but not that another
    /// player has taken over. Every report has to be passed in, in order.
    pub fn media(&mut self, playing: bool, session: Option<u64>, now: Millis) -> Option<EarAction> {
        self.expire(now);
        self.playing = playing;
        self.session = session;
        if let Some(mut paused) = self.paused {
            let other_player = session.is_some_and(|session| session != paused.session);
            if other_player || (playing && paused.stop_seen) {
                // Another player, or one that was stopped and plays again:
                // someone else is in charge of the music.
                self.paused = None;
            } else {
                // Still playing before it was seen to stop is a slow player.
                paused.stop_seen |= !playing;
                self.paused = Some(paused);
            }
        }
        if playing {
            return None;
        }
        self.resume()
    }

    /// The player refused to pause, or the pause could not be done. Whoever
    /// pauses it later is the user's doing.
    pub fn pause_refused(&mut self, session: u64) {
        if self.paused.is_some_and(|paused| paused.session == session) {
            self.paused = None;
        }
    }

    /// Whether the default audio output is these AirPods, as of now. When it
    /// is not, nothing is paused or resumed, and what was paused is let go: the
    /// sound moved elsewhere, and so is not this's to start again.
    pub fn output(&mut self, airpods: bool) {
        self.output_airpods = airpods;
        if !airpods {
            self.paused = None;
        }
    }

    /// The user took over: pressed play, pause, next or previous, or the
    /// followed player changed. What was paused is no longer this's to start.
    pub fn user_acted(&mut self) {
        self.paused = None;
    }

    /// The earbuds are gone, so everything is forgotten. Nothing starts again
    /// after a disconnect.
    pub fn disconnected(&mut self) {
        self.confirmed = None;
        self.candidate = None;
        self.paused = None;
    }

    fn expire(&mut self, now: Millis) {
        if let Some(paused) = self.paused {
            let age = now.saturating_sub(paused.at);
            if age > RESUME_WITHIN_MS || (!paused.stop_seen && age >= PAUSE_SETTLE_MS) {
                self.paused = None;
            }
        }
    }

    /// Fewer earbuds are worn than before. The music is paused if it plays,
    /// and it is only paused once: another earbud coming out while it is
    /// still stopping changes only what it takes to start it again.
    fn pause(&mut self, before: u8, now: Millis) -> Option<EarAction> {
        if !self.playing || !self.output_airpods {
            return None;
        }
        if let Some(paused) = &mut self.paused {
            paused.before = paused.before.max(before);
            return None;
        }
        let session = self.session?;
        self.paused = Some(Paused {
            before,
            at: now,
            session,
            stop_seen: false,
        });
        Some(EarAction::Pause(session))
    }

    /// Back to as many earbuds as when this paused, and the player seen to
    /// have stopped and not playing.
    fn resume(&mut self) -> Option<EarAction> {
        let paused = self.paused?;
        let back = self.confirmed.is_some_and(|count| count >= paused.before);
        if !back || self.playing || !paused.stop_seen || !self.output_airpods {
            return None;
        }
        self.paused = None;
        Some(EarAction::Resume(paused.session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The session of the player in most tests.
    const S: u64 = 7;

    /// Reports the count twice, as far apart as it takes to confirm it, and
    /// returns what the last one said.
    fn settle(control: &mut EarControl, count: u8, at: Millis) -> Option<EarAction> {
        assert_eq!(control.worn(Some(count), at), None, "first report");
        control.worn(Some(count), at + SETTLE_MS)
    }

    /// A control for the case that the sound goes to the AirPods.
    fn control() -> EarControl {
        let mut control = EarControl::new();
        control.output(true);
        control
    }

    /// A control with two earbuds worn and music playing.
    fn listening() -> EarControl {
        let mut control = control();
        assert_eq!(control.worn(Some(2), 0), None);
        assert_eq!(control.media(true, Some(S), 0), None);
        control
    }

    /// Pauses at 1.7 s by one earbud coming out, and has the player stop.
    fn paused() -> EarControl {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        assert_eq!(control.media(false, Some(S), 2_500), None);
        control
    }

    #[test]
    fn the_first_count_only_says_where_things_stand() {
        let mut control = control();
        control.media(true, Some(S), 0);
        assert_eq!(control.worn(Some(1), 0), None);
        assert_eq!(control.worn(Some(1), 10_000), None);
    }

    #[test]
    fn nothing_known_changes_nothing() {
        let mut control = listening();
        assert_eq!(control.worn(None, 1_000), None);
        assert_eq!(settle(&mut control, 1, 2_000), Some(EarAction::Pause(S)));
    }

    #[test]
    fn a_flicker_is_ignored() {
        let mut control = listening();
        assert_eq!(control.worn(Some(1), 1_000), None);
        // Back before it settled.
        assert_eq!(control.worn(Some(2), 1_300), None);
        assert_eq!(control.worn(Some(2), 3_000), None);
        // A different new count restarts the wait.
        assert_eq!(control.worn(Some(1), 4_000), None);
        assert_eq!(control.worn(Some(0), 4_500), None);
        assert_eq!(control.worn(Some(0), 4_500 + SETTLE_MS - 1), None);
        assert_eq!(
            control.worn(Some(0), 4_500 + SETTLE_MS),
            Some(EarAction::Pause(S))
        );
    }

    #[test]
    fn a_count_needs_a_later_report_to_hold() {
        let mut control = listening();
        // One report alone, however long ago, confirms nothing.
        assert_eq!(control.worn(Some(1), 100_000), None);
        assert_eq!(control.worn(Some(1), 100_000 + SETTLE_MS - 1), None);
        assert_eq!(
            control.worn(Some(1), 100_000 + SETTLE_MS),
            Some(EarAction::Pause(S))
        );
    }

    #[test]
    fn one_earbud_coming_out_pauses() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
    }

    #[test]
    fn both_coming_out_pauses_once_even_before_the_player_stops() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        // The second one comes out while the player still says it plays.
        assert_eq!(settle(&mut control, 0, 1_200), None);
        // The player stops, and nothing is paused again.
        assert_eq!(control.media(false, Some(S), 2_000), None);
        assert_eq!(control.worn(Some(0), 9_000), None);
        // Starting again takes both earbuds, as it did before the first.
        assert_eq!(settle(&mut control, 1, 10_000), None);
        assert_eq!(settle(&mut control, 2, 12_000), Some(EarAction::Resume(S)));
    }

    #[test]
    fn a_second_drop_does_not_move_the_time_of_the_pause() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        // Another one comes out at 2.4 s, still before the player stopped.
        assert_eq!(settle(&mut control, 0, 1_700), None);
        assert_eq!(control.media(false, Some(S), 2_500), None);
        // The window is counted from the pause at 1.7 s, so a change that
        // settles just past it is too late, though it would not be from 2.4 s.
        let end = 1_700 + RESUME_WITHIN_MS;
        assert_eq!(control.worn(Some(2), end - 600), None);
        assert_eq!(control.worn(Some(2), end + 100), None);
    }

    #[test]
    fn nothing_is_paused_twice_in_a_row() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        assert_eq!(control.worn(Some(1), 2_000), None);
        assert_eq!(control.worn(Some(1), 3_000), None);
    }

    #[test]
    fn going_back_in_resumes() {
        let mut control = paused();
        assert_eq!(settle(&mut control, 2, 5_000), Some(EarAction::Resume(S)));
        // Once.
        assert_eq!(control.worn(Some(2), 9_000), None);
        control.media(true, Some(S), 9_500);
        assert_eq!(control.media(false, Some(S), 20_000), None);
    }

    #[test]
    fn it_resumes_only_at_as_many_earbuds_as_before() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 0, 1_000), Some(EarAction::Pause(S)));
        control.media(false, Some(S), 2_500);
        // One earbud in is not as many as before.
        assert_eq!(settle(&mut control, 1, 5_000), None);
        assert_eq!(settle(&mut control, 2, 8_000), Some(EarAction::Resume(S)));
    }

    #[test]
    fn it_resumes_once_the_player_has_been_seen_to_stop() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        // The earbud is back while the player still says it plays.
        assert_eq!(settle(&mut control, 2, 1_100), None);
        // Then it says that it stopped.
        assert_eq!(
            control.media(false, Some(S), 2_900),
            Some(EarAction::Resume(S))
        );
        assert_eq!(control.media(false, Some(S), 3_000), None);
    }

    #[test]
    fn music_the_user_paused_is_not_started() {
        let mut control = listening();
        control.media(false, Some(S), 500);
        assert_eq!(settle(&mut control, 1, 1_000), None);
        assert_eq!(settle(&mut control, 2, 5_000), None);
        assert_eq!(control.media(false, Some(S), 6_000), None);
    }

    #[test]
    fn a_press_in_between_stops_the_resume() {
        let mut control = paused();
        control.user_acted();
        assert_eq!(settle(&mut control, 2, 5_000), None);
        assert_eq!(control.media(false, Some(S), 6_000), None);
    }

    #[test]
    fn music_that_starts_by_itself_is_left_alone() {
        let mut control = paused();
        // Someone starts it again, with an earbud still out.
        control.media(true, Some(S), 6_000);
        control.media(false, Some(S), 9_000);
        assert_eq!(settle(&mut control, 2, 10_000), None);
    }

    #[test]
    fn playing_again_right_after_it_stopped_is_the_user() {
        let mut control = paused();
        // Within 2 s of the pause, but after it was seen to stop.
        control.media(true, Some(S), 3_000);
        control.media(false, Some(S), 3_500);
        assert_eq!(settle(&mut control, 2, 5_000), None);
    }

    #[test]
    fn a_player_that_is_slow_to_stop_is_not_the_user_starting_it() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        // Still reported as playing right after the pause.
        control.media(true, Some(S), 1_800);
        control.media(false, Some(S), 2_500);
        assert_eq!(settle(&mut control, 2, 5_000), Some(EarAction::Resume(S)));
    }

    #[test]
    fn a_pause_that_did_not_work_is_forgotten_after_a_while() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        // It still plays 2 s later, and then the user pauses it by hand.
        control.media(true, Some(S), 3_800);
        control.media(false, Some(S), 5_000);
        assert_eq!(settle(&mut control, 2, 8_000), None);
    }

    #[test]
    fn a_pause_that_was_never_followed_by_a_stop_is_forgotten() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        // Nothing is heard from the player at all, then the user pauses it.
        assert_eq!(control.worn(Some(1), 5_000), None);
        control.media(false, Some(S), 6_000);
        assert_eq!(settle(&mut control, 2, 8_000), None);
    }

    #[test]
    fn a_refused_pause_is_forgotten_at_once() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        control.pause_refused(S);
        // The user pauses it by hand a moment later.
        control.media(false, Some(S), 2_000);
        assert_eq!(settle(&mut control, 2, 5_000), None);
    }

    #[test]
    fn a_refusal_from_another_session_changes_nothing() {
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        control.pause_refused(S + 1);
        control.media(false, Some(S), 2_000);
        assert_eq!(settle(&mut control, 2, 5_000), Some(EarAction::Resume(S)));
    }

    #[test]
    fn another_player_is_never_resumed() {
        let mut control = paused();
        // Another app's session becomes the followed one, and is not playing.
        control.media(false, Some(S + 1), 3_000);
        assert_eq!(settle(&mut control, 2, 5_000), None);
    }

    #[test]
    fn a_report_without_a_session_does_not_forget_what_was_paused() {
        let mut control = paused();
        // Nothing plays for a moment, as when the player says it stopped.
        control.media(false, None, 3_000);
        assert_eq!(settle(&mut control, 2, 5_000), Some(EarAction::Resume(S)));
    }

    #[test]
    fn a_report_without_a_session_is_not_paused() {
        let mut control = control();
        control.worn(Some(2), 0);
        // Playing, but no session said: it cannot be addressed.
        control.media(true, None, 0);
        assert_eq!(settle(&mut control, 1, 1_000), None);
    }

    #[test]
    fn a_player_that_is_not_playing_is_not_paused() {
        let mut control = control();
        control.worn(Some(2), 0);
        control.media(false, Some(S), 0);
        assert_eq!(settle(&mut control, 1, 1_000), None);
        assert_eq!(settle(&mut control, 2, 5_000), None);
        assert_eq!(control.media(false, Some(S), 6_000), None);
    }

    #[test]
    fn listening_with_one_earbud_pauses_and_resumes() {
        let mut control = control();
        control.worn(Some(1), 0);
        control.media(true, Some(S), 0);
        assert_eq!(settle(&mut control, 0, 1_000), Some(EarAction::Pause(S)));
        control.media(false, Some(S), 2_500);
        assert_eq!(settle(&mut control, 1, 5_000), Some(EarAction::Resume(S)));
    }

    #[test]
    fn it_does_not_resume_after_too_long() {
        let mut control = paused();
        let late = 1_000 + SETTLE_MS + RESUME_WITHIN_MS + 1;
        assert_eq!(settle(&mut control, 2, late), None);
        assert_eq!(control.media(false, Some(S), late + 5_000), None);
    }

    #[test]
    fn it_resumes_up_to_the_end_of_the_window() {
        let mut control = paused();
        // The pause was decided when the change settled.
        let paused_at = 1_000 + SETTLE_MS;
        let end = paused_at + RESUME_WITHIN_MS;
        assert_eq!(control.worn(Some(2), end - SETTLE_MS), None);
        assert_eq!(control.worn(Some(2), end), Some(EarAction::Resume(S)));
    }

    #[test]
    fn a_disconnect_forgets_everything() {
        let mut control = paused();
        control.disconnected();
        // The earbuds come back: the first count is only where things stand.
        assert_eq!(control.worn(Some(2), 5_000), None);
        assert_eq!(control.worn(Some(2), 9_000), None);
        assert_eq!(control.media(false, Some(S), 10_000), None);
    }

    #[test]
    fn a_lower_count_from_any_cause_pauses_and_its_return_resumes() {
        // Why the count is lower, say an earbud in the case, is the tracker's
        // business: its own tests show that a charging earbud is not counted
        // as worn.
        let mut control = listening();
        assert_eq!(settle(&mut control, 1, 1_000), Some(EarAction::Pause(S)));
        control.media(false, Some(S), 2_500);
        assert_eq!(settle(&mut control, 2, 5_000), Some(EarAction::Resume(S)));
    }

    #[test]
    fn nothing_is_paused_while_the_sound_goes_elsewhere() {
        let mut control = EarControl::new();
        // Where the sound goes is not known until it is said.
        control.worn(Some(2), 0);
        control.media(true, Some(S), 0);
        assert_eq!(settle(&mut control, 1, 1_000), None);
        control.output(false);
        assert_eq!(settle(&mut control, 0, 3_000), None);
    }

    #[test]
    fn nothing_is_resumed_while_the_sound_goes_elsewhere() {
        let mut control = paused();
        // The sound moved to the speakers while an earbud was out.
        control.output(false);
        assert_eq!(settle(&mut control, 2, 5_000), None);
        // Even back to the AirPods, what was paused is let go.
        control.output(true);
        assert_eq!(control.worn(Some(2), 9_000), None);
        assert_eq!(control.media(false, Some(S), 10_000), None);
    }

    #[test]
    fn the_sound_coming_to_the_airpods_later_pauses_on_the_next_drop() {
        let mut control = EarControl::new();
        control.worn(Some(2), 0);
        control.media(true, Some(S), 0);
        assert_eq!(settle(&mut control, 1, 1_000), None);
        assert_eq!(settle(&mut control, 2, 3_000), None);
        control.output(true);
        // No drop is made up for the time the sound was elsewhere.
        assert_eq!(control.worn(Some(2), 5_000), None);
        assert_eq!(settle(&mut control, 1, 6_000), Some(EarAction::Pause(S)));
    }

    #[test]
    fn a_clock_that_runs_backwards_does_not_panic() {
        let mut control = listening();
        control.worn(Some(1), 10_000);
        assert_eq!(control.worn(Some(1), 0), None);
        control.media(false, Some(S), 0);
        assert_eq!(control.worn(Some(2), 0), None);
    }
}

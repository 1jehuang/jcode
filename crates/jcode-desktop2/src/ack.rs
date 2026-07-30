//! Delivery state for the user's own messages, and the wiggle that reports it.
//!
//! A message typed into this app is not sent to the agent directly: it goes
//! down a socket, through the bridge, into the daemon, and only then into the
//! session's queue. Every one of those hops can be slow (a reconnect, a busy
//! agent, a daemon mid-rebuild), and until the reply's first token arrives the
//! UI has no proof the message landed at all. "It is on screen" and "the agent
//! has it" are different facts, and conflating them is what makes a stalled
//! turn indistinguishable from a dropped message.
//!
//! So a user message carries a [`Delivery`]: `Queued` while this window is
//! holding it back (the agent is mid-turn, and the daemon rejects a second
//! message outright), `Sent` the moment it leaves the composer, `Acked` when
//! the daemon confirms the agent took it (`ApiEvent::MessageAccepted`). The
//! difference is shown twice over, because one channel is not enough: a small
//! dot beside the card (hollow while pending, solid once acked) is the
//! *state*, and a short damped wiggle of the card is the *transition*. Motion
//! is what the eye notices without looking, which is exactly the right weight
//! for "it arrived". A queued card is additionally drawn in a fainter tone,
//! because "this has not gone anywhere yet" is a bigger claim than a dot.
//!
//! Like the rest of this app's animation, the wiggle is derived from
//! (state, now) rather than driven by a timer: a frame stays a pure function of
//! the model, and a capture can pin it by choosing the timestamp.

use std::time::{Duration, Instant};

/// How long the acknowledgement wiggle lasts. Long enough to read as a
/// deliberate nod, short enough that it is over before the reply starts
/// arriving.
pub const WIGGLE: Duration = Duration::from_millis(420);

/// Peak horizontal travel of the wiggle, in logical pixels. Deliberately
/// small: this is a confirmation, not an alert, and a card that visibly lurches
/// across the column would be the loudest thing on the page.
pub const WIGGLE_AMPLITUDE: f64 = 5.0;

/// Oscillations over the wiggle's life. Two full swings reads as a nod; more
/// reads as a shudder.
const WIGGLE_CYCLES: f64 = 2.0;

/// Frame interval requested while a wiggle runs, so the motion is paced by the
/// display rather than by whatever else happens to want frames.
pub const FRAME: Duration = Duration::from_millis(8);

/// Radius of the delivery dot, in logical pixels.
pub const DOT_RADIUS: f64 = 2.6;

/// Gap between the dot and the card's right text edge.
pub const DOT_GAP: f64 = 7.0;

/// Opacity of a message the agent has not confirmed yet: queued in this
/// window, or written to the socket without an acknowledgement. Faint enough
/// to read as "not landed", solid enough to stay legible, because the text
/// may be waiting a whole turn.
pub const PENDING_TONE: f64 = 0.55;

/// Where a user's message is on its way to the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Held in this window until the current turn finishes. The daemon
    /// refuses a message while it is processing one, so sending now would
    /// only earn an error; the message waits at the tail of the transcript
    /// instead, visibly not yet anyone's but ours.
    Queued,
    /// Handed to the connection; the agent has not confirmed it yet.
    Sent,
    /// The agent has the message. Carries when that was known, so the wiggle
    /// can be derived rather than scheduled.
    Acked { at: Instant },
}

impl Delivery {
    pub fn is_acked(self) -> bool {
        matches!(self, Self::Acked { .. })
    }

    /// Opacity of the message's card and text this frame.
    ///
    /// A message the agent does not have yet is drawn at [`PENDING_TONE`]:
    /// visibly on the page, visibly not yet part of the conversation. The
    /// acknowledgement ramps it to full ink over the wiggle's own duration,
    /// so the nod and the tone change read as one event, and a missed frame
    /// lands on solid ink rather than leaving the card washed out.
    pub fn tone(self, now: Instant) -> f64 {
        let Self::Acked { at } = self else {
            return PENDING_TONE;
        };
        let elapsed = now.saturating_duration_since(at).as_secs_f64();
        let span = WIGGLE.as_secs_f64();
        if elapsed >= span {
            return 1.0;
        }
        PENDING_TONE + (1.0 - PENDING_TONE) * (elapsed / span)
    }

    /// Horizontal offset for the card this frame, in logical pixels.
    ///
    /// A damped sine: it starts at zero, swings, and returns to zero, so the
    /// card ends exactly where it began and a missed frame cannot leave it
    /// displaced. `Sent` never moves; a pending message must not fidget.
    pub fn wiggle(self, now: Instant) -> f64 {
        let Self::Acked { at } = self else {
            return 0.0;
        };
        // A clock that went backwards (a pinned capture, a suspend) must read
        // as "not started" rather than panic in `duration_since`.
        let elapsed = now.saturating_duration_since(at).as_secs_f64();
        let span = WIGGLE.as_secs_f64();
        if elapsed >= span {
            return 0.0;
        }
        let t = elapsed / span;
        // (1 - t) damps the swing to nothing by the end; the sine gives the
        // swing itself, starting and finishing at zero crossings.
        let decay = 1.0 - t;
        WIGGLE_AMPLITUDE * decay * decay * (t * WIGGLE_CYCLES * std::f64::consts::TAU).sin()
    }

    /// When this message next needs a frame, or `None` when it is still.
    pub fn next_frame_at(self, now: Instant) -> Option<Instant> {
        let Self::Acked { at } = self else {
            return None;
        };
        (now.saturating_duration_since(at) < WIGGLE).then(|| now + FRAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_message_does_not_move_or_ask_for_frames() {
        let now = Instant::now();
        assert_eq!(Delivery::Sent.wiggle(now), 0.0);
        assert_eq!(Delivery::Sent.next_frame_at(now), None);
    }

    /// The wiggle must start and end at rest: a card left displaced by a
    /// dropped frame would be a permanent layout bug caused by an animation.
    #[test]
    fn the_wiggle_starts_and_ends_at_zero() {
        let at = Instant::now();
        let delivery = Delivery::Acked { at };
        assert_eq!(delivery.wiggle(at), 0.0);
        assert_eq!(delivery.wiggle(at + WIGGLE), 0.0);
        assert_eq!(delivery.wiggle(at + WIGGLE * 3), 0.0);
    }

    /// It has to actually move, and stay within its stated amplitude.
    #[test]
    fn the_wiggle_swings_within_its_amplitude() {
        let at = Instant::now();
        let delivery = Delivery::Acked { at };
        let mut peak: f64 = 0.0;
        for step in 0..64 {
            let now = at + WIGGLE.mul_f64(f64::from(step) / 64.0);
            peak = peak.max(delivery.wiggle(now).abs());
        }
        assert!(peak > 0.5, "the wiggle never moved: peak {peak}");
        assert!(
            peak <= WIGGLE_AMPLITUDE,
            "the wiggle overshot its amplitude: {peak}"
        );
    }

    /// Damping means the second half of the motion is smaller than the first,
    /// which is what makes it read as settling rather than as a loop.
    #[test]
    fn the_wiggle_decays() {
        let at = Instant::now();
        let delivery = Delivery::Acked { at };
        let early = delivery.wiggle(at + WIGGLE.mul_f64(0.125)).abs();
        let late = delivery.wiggle(at + WIGGLE.mul_f64(0.625)).abs();
        assert!(late < early, "early {early} late {late}");
    }

    /// Frames are requested only while the motion is live, so an idle window
    /// sleeps once every message has settled.
    #[test]
    fn frames_are_requested_only_while_the_wiggle_runs() {
        let at = Instant::now();
        let delivery = Delivery::Acked { at };
        assert!(delivery.next_frame_at(at).is_some());
        assert!(delivery.next_frame_at(at + WIGGLE / 2).is_some());
        assert!(delivery.next_frame_at(at + WIGGLE).is_none());
    }
}

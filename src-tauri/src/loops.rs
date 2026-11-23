use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const MAX_HOPS: u32 = 3;
pub const WINDOW: Duration = Duration::from_secs(60);
pub const MAX_TRACKED: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Proceed,
    Stop,
    StopAndAnnounce,
}

#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub started: Instant,
    pub finished: Instant,
}

#[derive(Debug, Clone, Copy)]
struct Placement {
    hops: u32,
    last_hop: Instant,
    announced: bool,
}

#[derive(Debug, Default)]
pub struct LoopGuard {
    placed: HashMap<PathBuf, Placement>,
    announced_full: bool,
}

impl LoopGuard {
    pub fn admit(&mut self, file: &Path, now: Instant) -> Admission {
        let Some(placement) = self.live(file, now) else {
            return self.admit_untracked(now);
        };
        if placement.hops < MAX_HOPS {
            return Admission::Proceed;
        }
        if placement.announced {
            return Admission::Stop;
        }
        placement.announced = true;
        Admission::StopAndAnnounce
    }

    pub fn copied(&mut self, from: &Path, to: &Path, span: Span) {
        let hop = self.next_hop(from, span);
        self.remember(to, hop, span.finished);
    }

    pub fn moved(&mut self, from: &Path, to: &Path, span: Span) {
        let hop = self.next_hop(from, span);
        self.placed.remove(from);
        self.remember(to, hop, span.finished);
    }

    // WHY: with memory full a new hop could not be counted, so a file nobody tracks waits for entries to expire
    fn admit_untracked(&mut self, now: Instant) -> Admission {
        if !self.is_full(now) {
            self.announced_full = false;
            return Admission::Proceed;
        }
        if std::mem::replace(&mut self.announced_full, true) {
            return Admission::Stop;
        }
        Admission::StopAndAnnounce
    }

    fn is_full(&mut self, now: Instant) -> bool {
        if self.placed.len() >= MAX_TRACKED {
            self.forget_expired(now);
        }
        self.placed.len() >= MAX_TRACKED
    }

    // WHY: the chain is read as the hop starts and stamped as it lands, so a slow move never outruns the window
    fn next_hop(&mut self, from: &Path, span: Span) -> Placement {
        let hops = self
            .live(from, span.started)
            .map_or(1, |prior| prior.hops.saturating_add(1));
        Placement {
            hops,
            last_hop: span.finished,
            announced: false,
        }
    }

    fn live(&mut self, file: &Path, now: Instant) -> Option<&mut Placement> {
        if self
            .placed
            .get(file)
            .is_some_and(|placement| has_expired(placement, now))
        {
            self.placed.remove(file);
        }
        self.placed.get_mut(file)
    }

    // WHY: forgetting a live hop would let its loop run on, so only expired entries ever make room
    fn remember(&mut self, to: &Path, hop: Placement, now: Instant) {
        self.placed.insert(to.to_path_buf(), hop);
        if self.placed.len() > MAX_TRACKED {
            self.forget_expired(now);
        }
    }

    fn forget_expired(&mut self, now: Instant) {
        self.placed
            .retain(|_, placement| !has_expired(placement, now));
    }
}

fn has_expired(placement: &Placement, now: Instant) -> bool {
    now.saturating_duration_since(placement.last_hop) >= WINDOW
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const HOP: Duration = Duration::from_secs(2);

    fn at(now: Instant) -> Span {
        Span {
            started: now,
            finished: now,
        }
    }

    fn leg(hop: u32) -> (&'static Path, &'static Path) {
        if hop.is_multiple_of(2) {
            (Path::new("/a/x"), Path::new("/b/x"))
        } else {
            (Path::new("/b/x"), Path::new("/a/x"))
        }
    }

    fn bounce_every(guard: &mut LoopGuard, hops: u32, start: Instant, every: Duration) -> Instant {
        (0..hops).fold(start, |now, hop| {
            let (from, to) = leg(hop);
            guard.moved(from, to, at(now));
            now + every
        })
    }

    fn bounce(guard: &mut LoopGuard, hops: u32, start: Instant) -> Instant {
        bounce_every(guard, hops, start, HOP)
    }

    fn last_stop(hops: u32) -> &'static Path {
        Path::new(if hops.is_multiple_of(2) {
            "/a/x"
        } else {
            "/b/x"
        })
    }

    fn fill(guard: &mut LoopGuard, hops: u32, from: Instant) -> Instant {
        (0..MAX_TRACKED).fold(from, |now, index| {
            let stops: Vec<PathBuf> = (0..=hops)
                .map(|stop| PathBuf::from(format!("/fill/{index}/{stop}")))
                .collect();
            for pair in stops.windows(2) {
                guard.moved(&pair[0], &pair[1], at(now));
            }
            now + Duration::from_micros(1)
        })
    }

    fn fill_with_stopped_loops(guard: &mut LoopGuard, from: Instant) -> Instant {
        let filled = fill(guard, MAX_HOPS, from);
        for index in 0..MAX_TRACKED {
            let stopped = PathBuf::from(format!("/fill/{index}/{MAX_HOPS}"));
            assert_eq!(guard.admit(&stopped, filled), Admission::StopAndAnnounce);
        }
        filled
    }

    fn one_round(
        guard: &mut LoopGuard,
        files: usize,
        round: u32,
        start: Instant,
    ) -> Vec<Admission> {
        let step = Duration::from_millis(10);
        (0..files)
            .map(|index| {
                let now = start + step * u32::try_from(index).unwrap();
                let (from, to) = (side(index, round), side(index, round + 1));
                let answer = guard.admit(&from, now);
                if answer == Admission::Proceed {
                    guard.moved(&from, &to, at(now));
                }
                answer
            })
            .collect()
    }

    fn side(index: usize, hop: u32) -> PathBuf {
        let folder = if hop.is_multiple_of(2) { "a" } else { "b" };
        PathBuf::from(format!("/{folder}/{index:05}.txt"))
    }

    #[test]
    fn a_file_nobody_placed_is_let_through() {
        let mut guard = LoopGuard::default();

        assert_eq!(
            guard.admit(Path::new("/a/x"), Instant::now()),
            Admission::Proceed
        );
    }

    #[test]
    fn up_to_three_hops_go_through_and_the_fourth_is_stopped() {
        let start = Instant::now();
        for (hops, expected) in [
            (1, Admission::Proceed),
            (2, Admission::Proceed),
            (3, Admission::StopAndAnnounce),
        ] {
            let mut guard = LoopGuard::default();
            let now = bounce(&mut guard, hops, start);

            assert_eq!(
                guard.admit(last_stop(hops), now),
                expected,
                "after {hops} hops"
            );
        }
    }

    #[test]
    fn a_stopped_file_is_announced_once_per_window() {
        let mut guard = LoopGuard::default();
        let now = bounce(&mut guard, 3, Instant::now());

        let answers: Vec<Admission> = (0..4)
            .map(|_| guard.admit(Path::new("/b/x"), now))
            .collect();

        assert_eq!(
            answers,
            [
                Admission::StopAndAnnounce,
                Admission::Stop,
                Admission::Stop,
                Admission::Stop
            ]
        );
    }

    #[test]
    fn a_loop_of_hops_25_seconds_apart_is_still_stopped() {
        let mut guard = LoopGuard::default();

        let fourth = bounce_every(&mut guard, 3, Instant::now(), Duration::from_secs(25));

        assert_eq!(
            guard.admit(last_stop(3), fourth),
            Admission::StopAndAnnounce,
            "a window fixed at the first hop lets the fourth hop through at 75 s"
        );
    }

    #[test]
    fn a_hop_slower_than_the_window_still_counts() {
        let mut guard = LoopGuard::default();
        let (copying, quiet) = (WINDOW + Duration::from_secs(30), Duration::from_secs(2));

        let next = (0..3).fold(Instant::now(), |started, hop| {
            let (from, to) = leg(hop);
            let finished = started + copying;
            guard.moved(from, to, Span { started, finished });
            finished + quiet
        });

        assert_eq!(guard.admit(last_stop(3), next), Admission::StopAndAnnounce);
    }

    #[test]
    fn the_window_is_counted_from_the_last_hop() {
        for (quiet, expected) in [
            (WINDOW - Duration::from_secs(1), Admission::StopAndAnnounce),
            (WINDOW, Admission::Proceed),
        ] {
            let start = Instant::now();
            let mut guard = LoopGuard::default();
            bounce(&mut guard, 3, start);
            let last_hop = start + HOP * 2;

            assert_eq!(
                guard.admit(last_stop(3), last_hop + quiet),
                expected,
                "{quiet:?} after the last hop"
            );
        }
    }

    #[test]
    fn a_file_that_comes_back_after_a_quiet_window_starts_counting_again() {
        let mut guard = LoopGuard::default();
        let later = bounce(&mut guard, 3, Instant::now()) + WINDOW;

        let answers: Vec<Admission> = (3..7)
            .map(|hop| {
                let (from, to) = leg(hop);
                let answer = guard.admit(from, later);
                guard.moved(from, to, at(later));
                answer
            })
            .collect();

        assert_eq!(
            answers,
            [
                Admission::Proceed,
                Admission::Proceed,
                Admission::Proceed,
                Admission::StopAndAnnounce
            ]
        );
    }

    #[test]
    fn a_chain_through_several_folders_counts_every_hop() {
        let now = at(Instant::now());
        let mut guard = LoopGuard::default();
        guard.moved(Path::new("/downloads/x"), Path::new("/inbox/x"), now);
        guard.moved(Path::new("/inbox/x"), Path::new("/archive/x"), now);

        assert_eq!(
            guard.admit(Path::new("/archive/x"), now.finished),
            Admission::Proceed
        );
        guard.moved(Path::new("/archive/x"), Path::new("/old/x"), now);
        assert_eq!(
            guard.admit(Path::new("/old/x"), now.finished),
            Admission::StopAndAnnounce
        );
    }

    #[test]
    fn a_copy_hands_its_hops_on_and_the_original_keeps_its_own() {
        let now = at(Instant::now());
        let mut guard = LoopGuard::default();
        guard.copied(Path::new("/a/x"), Path::new("/b/x"), now);
        guard.copied(Path::new("/b/x"), Path::new("/a/x (2)"), now);
        guard.copied(Path::new("/a/x (2)"), Path::new("/b/x (2)"), now);

        assert_eq!(
            guard.admit(Path::new("/a/x"), now.finished),
            Admission::Proceed
        );
        assert_eq!(
            guard.admit(Path::new("/b/x (2)"), now.finished),
            Admission::StopAndAnnounce,
            "a copy loop multiplies files, so it counts like a bounce"
        );
    }

    #[test]
    fn a_new_file_where_one_moved_away_starts_from_zero() {
        let now = Instant::now();
        let mut guard = LoopGuard::default();
        bounce(&mut guard, 3, now);
        guard.moved(Path::new("/b/x"), Path::new("/c/x"), at(now));

        assert_eq!(guard.admit(Path::new("/b/x"), now), Admission::Proceed);
    }

    #[test]
    fn a_ping_pong_of_1500_files_in_sorted_order_stops_every_file_at_its_fourth_hop() {
        let mut guard = LoopGuard::default();
        let start = Instant::now();

        let rounds: Vec<Vec<Admission>> = (0..4)
            .map(|round| {
                let begins = start + Duration::from_secs(20) * round;
                one_round(&mut guard, 1_500, round, begins)
            })
            .collect();

        for (round, answers) in rounds.iter().enumerate() {
            let expected = if round < 3 {
                Admission::Proceed
            } else {
                Admission::StopAndAnnounce
            };
            let wrong = answers.iter().filter(|answer| **answer != expected).count();
            assert_eq!(wrong, 0, "round {round}: {wrong} files answered otherwise");
        }
    }

    #[test]
    fn when_memory_is_full_a_file_nobody_tracks_is_held_and_it_is_announced_once() {
        let mut guard = LoopGuard::default();
        let filled = fill(&mut guard, 1, Instant::now());

        let answers: Vec<Admission> = ["/new/1", "/new/2"]
            .iter()
            .map(|file| guard.admit(Path::new(file), filled))
            .collect();

        assert_eq!(answers, [Admission::StopAndAnnounce, Admission::Stop]);
        assert_eq!(
            guard.admit(Path::new("/fill/0/1"), filled),
            Admission::Proceed,
            "a file already tracked still counts its own hops"
        );
    }

    #[test]
    fn memory_frees_up_once_the_window_has_passed() {
        let mut guard = LoopGuard::default();
        let filled = fill(&mut guard, 1, Instant::now());
        assert_eq!(
            guard.admit(Path::new("/new/1"), filled),
            Admission::StopAndAnnounce
        );

        let later = filled + WINDOW;

        assert_eq!(guard.admit(Path::new("/new/1"), later), Admission::Proceed);
        assert!(guard.placed.is_empty(), "expired entries were kept");
    }

    #[test]
    fn a_hop_just_recorded_is_never_forgotten_to_make_room() {
        let start = Instant::now();
        let mut guard = LoopGuard::default();
        bounce(&mut guard, 2, start);
        let filled = fill_with_stopped_loops(&mut guard, start);
        let (from, to) = leg(2);

        assert_eq!(guard.admit(from, filled), Admission::Proceed);
        guard.moved(from, to, at(filled));

        assert_eq!(
            guard.admit(to, filled),
            Admission::StopAndAnnounce,
            "the third hop was forgotten as soon as it was recorded"
        );
    }

    #[test]
    fn a_stopped_loop_is_never_forgotten_to_make_room_before_its_window_ends() {
        let mut guard = LoopGuard::default();
        let now = bounce(&mut guard, 3, Instant::now());
        assert_eq!(
            guard.admit(Path::new("/b/x"), now),
            Admission::StopAndAnnounce
        );

        fill(&mut guard, 3, now);

        assert_eq!(
            guard.admit(Path::new("/b/x"), now + HOP),
            Admission::Stop,
            "forgetting the stopped file would let the loop start again"
        );
    }
}

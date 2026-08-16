//! Driver edge cases: executor shutdown mid-flight, contained panics, the
//! Skip-overlap-with-an-out-of-band-tick path, and exhausted cron schedules.

mod arming;
mod next_run;
mod overlap;
mod panics;
mod resume;
mod shutdown;
mod support;
mod trigger_now;

use std::time::{Duration, Instant};

/// Progress tracker for build execution.
pub struct Progress {
    total: usize,
    completed: usize,
    failed: usize,
    start: Instant,
    active_jobs: usize,
}

impl Progress {
    /// Create a new progress tracker.
    pub fn new(total: usize) -> Self {
        Progress {
            total,
            completed: 0,
            failed: 0,
            start: Instant::now(),
            active_jobs: 0,
        }
    }

    /// Mark a job as started.
    pub fn job_started(&mut self) {
        self.active_jobs += 1;
    }

    /// Mark a job as completed.
    pub fn job_completed(&mut self, success: bool, duration: Duration) {
        self.completed += 1;
        self.active_jobs = self.active_jobs.saturating_sub(1);
        if !success {
            self.failed += 1;
        }
        let _ = duration; // Available for future per-job timing
    }

    /// Get a formatted status line.
    pub fn status_line(&self) -> String {
        let elapsed = self.start.elapsed();
        let pct = if self.total > 0 {
            (self.completed * 100) / self.total
        } else {
            0
        };

        format!(
            "[{}/{}] {}% {:.1}s",
            self.completed,
            self.total,
            pct,
            elapsed.as_secs_f64(),
        )
    }

    /// Get the total number of commands.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Get the number of completed commands.
    pub fn completed(&self) -> usize {
        self.completed
    }

    /// Get the number of failed commands.
    pub fn failed(&self) -> usize {
        self.failed
    }

    /// Get the elapsed time.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Format the final summary.
    pub fn summary(&self) -> String {
        let elapsed = self.elapsed();
        if self.failed > 0 {
            format!(
                "{} command(s) failed out of {} ({:.1}s)",
                self.failed, self.total, elapsed.as_secs_f64()
            )
        } else {
            format!(
                "{} command(s) ran successfully ({:.1}s)",
                self.completed, elapsed.as_secs_f64()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_basic() {
        let mut p = Progress::new(3);
        assert_eq!(p.total(), 3);
        assert_eq!(p.completed(), 0);

        p.job_started();
        p.job_completed(true, Duration::from_millis(10));
        assert_eq!(p.completed(), 1);
        assert_eq!(p.failed(), 0);
    }

    #[test]
    fn test_progress_with_failure() {
        let mut p = Progress::new(2);
        p.job_started();
        p.job_completed(true, Duration::from_millis(10));
        p.job_started();
        p.job_completed(false, Duration::from_millis(5));

        assert_eq!(p.completed(), 2);
        assert_eq!(p.failed(), 1);
    }

    #[test]
    fn test_progress_status_line() {
        let mut p = Progress::new(10);
        p.job_started();
        p.job_completed(true, Duration::from_millis(1));
        let status = p.status_line();
        assert!(status.contains("[1/10]"));
        assert!(status.contains("10%"));
    }

    #[test]
    fn test_progress_summary_success() {
        let mut p = Progress::new(3);
        for _ in 0..3 {
            p.job_started();
            p.job_completed(true, Duration::from_millis(1));
        }
        let summary = p.summary();
        assert!(summary.contains("3 command(s) ran successfully"));
    }

    #[test]
    fn test_progress_summary_failure() {
        let mut p = Progress::new(3);
        p.job_started();
        p.job_completed(true, Duration::from_millis(1));
        p.job_started();
        p.job_completed(false, Duration::from_millis(1));

        let summary = p.summary();
        assert!(summary.contains("1 command(s) failed"));
    }
}

//! Session-scoped scheduled tasks, reminders, and multi-round interval schedules.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskType {
    Notification,
    Command,
    Query,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IntervalPhase {
    Work,
    Break,
}

fn default_round_label() -> String {
    "Round".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntervalSchedule {
    /// Total number of rounds / laps (`>= 2`).
    pub total_rounds: u32,
    /// 1-based index of the active or just-completed round (`1..=total_rounds`).
    pub current_round: u32,
    /// Duration of each active round / lap in seconds.
    pub work_secs: u32,
    /// Rest / break duration between rounds in seconds (`0` for back-to-back laps).
    pub break_secs: u32,
    /// Current phase (`Work` vs `Break`).
    pub phase: IntervalPhase,
    /// UTC timestamp when the current phase started (drives UI progress bar).
    pub phase_started_at: DateTime<Utc>,
    /// Display unit for each round (e.g. `"Round"`, `"Lap"`, `"Pomodoro"`).
    #[serde(default = "default_round_label")]
    pub round_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduledTask {
    pub id: Uuid,
    pub task_type: TaskType,
    pub payload: String,
    pub due_at: DateTime<Utc>,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<IntervalSchedule>,
    /// Remaining milliseconds frozen when the user pauses a task/round (`None` = running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_remaining_ms: Option<i64>,
}

impl ScheduledTask {
    #[must_use]
    pub const fn is_paused(&self) -> bool {
        self.paused_remaining_ms.is_some()
    }

    /// Toggles pause/resume relative to `now`. Non-pending (`Completed` / `Cancelled`)
    /// tasks ignore pause requests so finished timers cannot be resurrected.
    pub fn toggle_pause_at(&mut self, now: DateTime<Utc>) {
        if self.status != TaskStatus::Pending {
            self.paused_remaining_ms = None;
            return;
        }

        if let Some(rem_ms) = self.paused_remaining_ms.take() {
            let clamped_ms = rem_ms.max(0);
            self.due_at = now + chrono::Duration::milliseconds(clamped_ms);
            if let Some(ref mut iv) = self.interval {
                let phase_secs = match iv.phase {
                    IntervalPhase::Work => iv.work_secs,
                    IntervalPhase::Break => iv.break_secs,
                }
                .max(1);
                let total_ms = i64::from(phase_secs).saturating_mul(1_000);
                let elapsed_ms = total_ms.saturating_sub(clamped_ms).clamp(0, total_ms);
                iv.phase_started_at = now - chrono::Duration::milliseconds(elapsed_ms);
            }
        } else {
            let rem_ms = self
                .due_at
                .signed_duration_since(now)
                .num_milliseconds()
                .max(0);
            self.paused_remaining_ms = Some(rem_ms);
        }
    }

    pub fn toggle_pause_now(&mut self) {
        self.toggle_pause_at(Utc::now());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingTaskInfo {
    pub id: Uuid,
    pub pane_id: String,
    pub session_id: String,
    pub character_id: String,
    pub character_name: String,
    pub character_slug: String,
    pub task_type: TaskType,
    pub payload: String,
    /// UTC Unix timestamp (seconds) when the task is due.
    pub due_at_unix: i64,
    /// Remaining seconds at snapshot time (`max(0, due_at - now)`).
    pub remaining_secs: i64,
    /// True if the owning pane is currently detached in the background.
    pub is_detached: bool,
    /// PTY PID associated with the pane, used to restore if detached.
    pub pid: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn pending_task_info_roundtrips_json() {
        let info = PendingTaskInfo {
            id: Uuid::new_v4(),
            pane_id: "pane-1".to_string(),
            session_id: "sess-1".to_string(),
            character_id: "char-1".to_string(),
            character_name: "Niko".to_string(),
            character_slug: "niko".to_string(),
            task_type: TaskType::Notification,
            payload: "Check build".to_string(),
            due_at_unix: 1_750_000_000,
            remaining_secs: 120,
            is_detached: true,
            pid: Some(4242),
        };
        let json = serde_json::to_string(&info).unwrap();
        let decoded: PendingTaskInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, decoded);
    }

    #[test]
    fn scheduled_task_pause_and_resume_preserves_interval_progress() {
        let t0 = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let mut task = ScheduledTask {
            id: Uuid::new_v4(),
            task_type: TaskType::Notification,
            payload: "Pomodoro".to_string(),
            due_at: t0 + chrono::Duration::seconds(60),
            status: TaskStatus::Pending,
            interval: Some(IntervalSchedule {
                total_rounds: 4,
                current_round: 1,
                work_secs: 60,
                break_secs: 15,
                phase: IntervalPhase::Work,
                phase_started_at: t0,
                round_label: "Round".to_string(),
            }),
            paused_remaining_ms: None,
        };

        // Pause 20s into the 60s work phase -> 40,000 ms remaining
        let t_pause = t0 + chrono::Duration::seconds(20);
        task.toggle_pause_at(t_pause);
        assert!(task.is_paused());
        assert_eq!(task.paused_remaining_ms, Some(40_000));

        // Resume 5 minutes later -> due_at shifts to t_resume + 40s, phase_started_at shifts to t_resume - 20s
        let t_resume = t_pause + chrono::Duration::minutes(5);
        task.toggle_pause_at(t_resume);
        assert!(!task.is_paused());
        assert_eq!(task.due_at, t_resume + chrono::Duration::seconds(40));
        assert_eq!(
            task.interval.as_ref().unwrap().phase_started_at,
            t_resume - chrono::Duration::seconds(20)
        );

        // Completed tasks ignore pause requests
        task.status = TaskStatus::Completed;
        task.toggle_pause_at(t_resume);
        assert!(!task.is_paused());
    }
}

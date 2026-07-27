use std::collections::HashSet;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Deck {
    pub schema_version: u32,
    pub id: Uuid,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub cards: Vec<Card>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Card {
    pub id: Uuid,
    pub front: String,
    pub back: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub stats: CardStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CardStats {
    #[serde(default)]
    pub again_count: u64,
    #[serde(default)]
    pub good_count: u64,
    #[serde(default)]
    pub current_good_streak: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reviewed: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<RatingEvent>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Rating {
    Again,
    Good,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RatingEvent {
    pub id: Uuid,
    pub session_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub local_date: NaiveDate,
    pub utc_offset_minutes: i32,
    pub rating: Rating,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StudyOrder {
    Sequential,
    Shuffled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionSummary {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub order: StudyOrder,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tag_filter: Vec<String>,
    pub again_count: u64,
    pub good_count: u64,
    pub unique_card_count: usize,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeckSummary {
    pub cards: usize,
    pub reviews: u64,
    pub good: u64,
    pub again: u64,
    pub success_rate: f64,
    pub last_studied: Option<DateTime<Utc>>,
    pub study_day_streak: u32,
}

impl Deck {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            title: title.into(),
            description: String::new(),
            cards: Vec::new(),
            sessions: Vec::new(),
        }
    }

    pub fn normalize(&mut self) {
        self.title = self.title.trim().to_owned();
        self.description = self.description.trim().to_owned();
        for card in &mut self.cards {
            card.tags = normalize_tags(card.tags.drain(..));
        }
    }

    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        if self.schema_version != SCHEMA_VERSION {
            issues.push(issue(
                "schema_version",
                format!(
                    "Unsupported schema version {} (expected {SCHEMA_VERSION})",
                    self.schema_version
                ),
            ));
        }
        if self.title.trim().is_empty() {
            issues.push(issue("title", "Deck title cannot be empty"));
        }

        let mut card_ids = HashSet::new();
        let mut event_ids = HashSet::new();
        for (index, card) in self.cards.iter().enumerate() {
            let base = format!("cards[{index}]");
            if !card_ids.insert(card.id) {
                issues.push(issue(format!("{base}.id"), "Duplicate card ID"));
            }
            if card.front.trim().is_empty() {
                issues.push(issue(format!("{base}.front"), "Front cannot be empty"));
            }
            if card.back.trim().is_empty() {
                issues.push(issue(format!("{base}.back"), "Back cannot be empty"));
            }
            if normalize_tags(card.tags.iter().cloned()) != card.tags {
                issues.push(issue(
                    format!("{base}.tags"),
                    "Tags must be trimmed, lowercase, sorted, and unique",
                ));
            }

            let again = card
                .stats
                .history
                .iter()
                .filter(|event| event.rating == Rating::Again)
                .count() as u64;
            let good = card
                .stats
                .history
                .iter()
                .filter(|event| event.rating == Rating::Good)
                .count() as u64;
            if again != card.stats.again_count || good != card.stats.good_count {
                issues.push(issue(
                    format!("{base}.stats"),
                    "Aggregate counts do not match rating history",
                ));
            }
            let last = card.stats.history.iter().map(|e| e.timestamp).max();
            if last != card.stats.last_reviewed {
                issues.push(issue(
                    format!("{base}.stats.last_reviewed"),
                    "Last-reviewed timestamp does not match rating history",
                ));
            }
            for (event_index, event) in card.stats.history.iter().enumerate() {
                if !event_ids.insert(event.id) {
                    issues.push(issue(
                        format!("{base}.stats.history[{event_index}].id"),
                        "Duplicate rating event ID",
                    ));
                }
                if !(-1_440..=1_440).contains(&event.utc_offset_minutes) {
                    issues.push(issue(
                        format!("{base}.stats.history[{event_index}].utc_offset_minutes"),
                        "UTC offset is out of range",
                    ));
                }
            }
        }

        let mut session_ids = HashSet::new();
        for (index, session) in self.sessions.iter().enumerate() {
            if !session_ids.insert(session.id) {
                issues.push(issue(
                    format!("sessions[{index}].id"),
                    "Duplicate session ID",
                ));
            }
            if session.completed_at < session.started_at {
                issues.push(issue(
                    format!("sessions[{index}].completed_at"),
                    "Completion cannot precede session start",
                ));
            }
            if !session.completed {
                issues.push(issue(
                    format!("sessions[{index}].completed"),
                    "Only completed sessions belong in session summaries",
                ));
            }
        }
        issues
    }

    pub fn record_rating(
        &mut self,
        card_id: Uuid,
        session_id: Uuid,
        rating: Rating,
        timestamp: DateTime<Utc>,
        offset: FixedOffset,
    ) -> Result<RatingEvent, String> {
        let card = self
            .cards
            .iter_mut()
            .find(|card| card.id == card_id)
            .ok_or_else(|| "The card no longer exists in this deck".to_owned())?;
        let event = RatingEvent {
            id: Uuid::new_v4(),
            session_id,
            timestamp,
            local_date: timestamp.with_timezone(&offset).date_naive(),
            utc_offset_minutes: offset.local_minus_utc() / 60,
            rating,
        };
        card.stats.history.push(event.clone());
        match rating {
            Rating::Again => {
                card.stats.again_count += 1;
                card.stats.current_good_streak = 0;
            }
            Rating::Good => {
                card.stats.good_count += 1;
                card.stats.current_good_streak += 1;
            }
        }
        card.stats.last_reviewed = Some(timestamp);
        Ok(event)
    }

    pub fn summary(&self, today: NaiveDate) -> DeckSummary {
        let mut summary = DeckSummary {
            cards: self.cards.len(),
            ..Default::default()
        };
        let mut study_days = HashSet::new();
        for event in self.cards.iter().flat_map(|card| card.stats.history.iter()) {
            summary.reviews += 1;
            match event.rating {
                Rating::Again => summary.again += 1,
                Rating::Good => summary.good += 1,
            }
            summary.last_studied = Some(
                summary
                    .last_studied
                    .map_or(event.timestamp, |last| last.max(event.timestamp)),
            );
            study_days.insert(event.local_date);
        }
        if summary.reviews > 0 {
            summary.success_rate = summary.good as f64 / summary.reviews as f64;
        }

        let mut day = today;
        if !study_days.contains(&day) {
            day = match day.pred_opt() {
                Some(previous) if study_days.contains(&previous) => previous,
                _ => return summary,
            };
        }
        while study_days.contains(&day) {
            summary.study_day_streak += 1;
            let Some(previous) = day.pred_opt() else {
                break;
            };
            day = previous;
        }
        summary
    }

    pub fn card_mut(&mut self, id: Uuid) -> Option<&mut Card> {
        self.cards.iter_mut().find(|card| card.id == id)
    }
}

impl Card {
    pub fn new(front: impl Into<String>, back: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            front: front.into(),
            back: back.into(),
            tags: Vec::new(),
            stats: CardStats::default(),
        }
    }
}

pub fn normalize_tags(tags: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut tags: Vec<_> = tags
        .into_iter()
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

fn issue(path: impl Into<String>, message: impl Into<String>) -> ValidationIssue {
    ValidationIssue {
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn ratings_update_history_and_aggregates() {
        let mut deck = Deck::new("Spanish");
        let card = Card::new("hola", "hello");
        let card_id = card.id;
        deck.cards.push(card);
        let timestamp = Utc.with_ymd_and_hms(2026, 7, 27, 12, 0, 0).unwrap();
        deck.record_rating(
            card_id,
            Uuid::new_v4(),
            Rating::Good,
            timestamp,
            FixedOffset::west_opt(7 * 3600).unwrap(),
        )
        .unwrap();

        assert!(deck.validate().is_empty());
        assert_eq!(deck.cards[0].stats.good_count, 1);
        assert_eq!(
            deck.cards[0].stats.history[0].local_date,
            NaiveDate::from_ymd_opt(2026, 7, 27).unwrap()
        );
    }

    #[test]
    fn streak_allows_today_or_yesterday_as_latest_day() {
        let mut deck = Deck::new("Deck");
        let mut card = Card::new("a", "b");
        for day in 25..=27 {
            card.stats.history.push(RatingEvent {
                id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                timestamp: Utc.with_ymd_and_hms(2026, 7, day, 18, 0, 0).unwrap(),
                local_date: NaiveDate::from_ymd_opt(2026, 7, day).unwrap(),
                utc_offset_minutes: -420,
                rating: Rating::Good,
            });
        }
        card.stats.good_count = 3;
        card.stats.current_good_streak = 3;
        card.stats.last_reviewed = card.stats.history.last().map(|event| event.timestamp);
        deck.cards.push(card);
        assert_eq!(
            deck.summary(NaiveDate::from_ymd_opt(2026, 7, 27).unwrap())
                .study_day_streak,
            3
        );
        assert_eq!(
            deck.summary(NaiveDate::from_ymd_opt(2026, 7, 28).unwrap())
                .study_day_streak,
            3
        );
    }

    #[test]
    fn validation_finds_aggregate_mismatch() {
        let mut deck = Deck::new("Deck");
        let mut card = Card::new("front", "back");
        card.stats.good_count = 2;
        deck.cards.push(card);
        assert!(
            deck.validate()
                .iter()
                .any(|issue| issue.path.ends_with(".stats"))
        );
    }

    #[test]
    fn tags_are_canonical() {
        assert_eq!(
            normalize_tags([" Grammar ", "verbs", "grammar"].map(str::to_owned)),
            vec!["grammar", "verbs"]
        );
    }
}

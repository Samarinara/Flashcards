use std::collections::{HashSet, VecDeque};

use chrono::{DateTime, Utc};
use rand::seq::SliceRandom;
use uuid::Uuid;

use crate::model::{Deck, Rating, SessionSummary, StudyOrder};

#[derive(Debug, Clone)]
pub struct StudySession {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub order: StudyOrder,
    pub tag_filter: Vec<String>,
    queue: VecDeque<Uuid>,
    requeued: HashSet<Uuid>,
    seen: HashSet<Uuid>,
    current: Option<Uuid>,
    again_count: u64,
    good_count: u64,
    initial_count: usize,
}

impl StudySession {
    pub fn new(deck: &Deck, order: StudyOrder, tag_filter: Vec<String>) -> Self {
        let mut cards: Vec<_> = deck
            .cards
            .iter()
            .filter(|card| {
                tag_filter.is_empty()
                    || tag_filter
                        .iter()
                        .all(|tag| card.tags.iter().any(|card_tag| card_tag == tag))
            })
            .map(|card| card.id)
            .collect();
        if order == StudyOrder::Shuffled {
            cards.shuffle(&mut rand::rng());
        }
        let initial_count = cards.len();
        let mut queue = VecDeque::from(cards);
        let current = queue.pop_front();
        Self {
            id: Uuid::new_v4(),
            started_at: Utc::now(),
            order,
            tag_filter,
            queue,
            requeued: HashSet::new(),
            seen: HashSet::new(),
            current,
            again_count: 0,
            good_count: 0,
            initial_count,
        }
    }

    pub fn current_card(&self) -> Option<Uuid> {
        self.current
    }

    pub fn progress(&self) -> (usize, usize) {
        let completed = self.seen.len().min(self.initial_count);
        (completed, self.initial_count)
    }

    pub fn rate(&mut self, rating: Rating) -> Option<Uuid> {
        let card_id = self.current?;
        self.seen.insert(card_id);
        match rating {
            Rating::Again => {
                self.again_count += 1;
                if self.requeued.insert(card_id) {
                    self.queue.push_back(card_id);
                }
            }
            Rating::Good => self.good_count += 1,
        }
        self.current = self.queue.pop_front();
        self.current
    }

    pub fn is_complete(&self) -> bool {
        self.current.is_none()
    }

    pub fn finish(self, completed_at: DateTime<Utc>) -> SessionSummary {
        SessionSummary {
            id: self.id,
            started_at: self.started_at,
            completed_at,
            order: self.order,
            tag_filter: self.tag_filter,
            again_count: self.again_count,
            good_count: self.good_count,
            unique_card_count: self.seen.len(),
            completed: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Card;

    #[test]
    fn again_requeues_exactly_once() {
        let mut deck = Deck::new("Deck");
        let card = Card::new("Front", "Back");
        let id = card.id;
        deck.cards.push(card);
        let mut session = StudySession::new(&deck, StudyOrder::Sequential, vec![]);
        assert_eq!(session.current_card(), Some(id));
        assert_eq!(session.rate(Rating::Again), Some(id));
        assert_eq!(session.rate(Rating::Again), None);
        assert!(session.is_complete());
    }

    #[test]
    fn tag_filter_requires_all_selected_tags() {
        let mut deck = Deck::new("Deck");
        let mut first = Card::new("A", "B");
        first.tags = vec!["easy".into(), "verbs".into()];
        let included = first.id;
        let mut second = Card::new("C", "D");
        second.tags = vec!["verbs".into()];
        deck.cards.extend([first, second]);
        let session = StudySession::new(
            &deck,
            StudyOrder::Sequential,
            vec!["easy".into(), "verbs".into()],
        );
        assert_eq!(session.current_card(), Some(included));
        assert_eq!(session.progress(), (0, 1));
    }
}

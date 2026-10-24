//! Back and forward through the places that go to definition jumped from.

const LIMIT: usize = 100;

#[derive(Debug)]
pub struct NavigationHistory<T> {
    back: Vec<T>,
    forward: Vec<T>,
}

impl<T> Default for NavigationHistory<T> {
    fn default() -> Self {
        Self {
            back: Vec::new(),
            forward: Vec::new(),
        }
    }
}

impl<T: PartialEq> NavigationHistory<T> {
    /// Remembers where a jump started. A new jump forgets the places ahead.
    pub fn record(&mut self, from: T) {
        self.forward.clear();
        if self.back.last() != Some(&from) {
            self.back.push(from);
        }
        if self.back.len() > LIMIT {
            self.back.remove(0);
        }
    }

    /// The place to go back to from `current`, which forward then returns to.
    pub fn back(&mut self, current: T) -> Option<T> {
        let target = self.back.pop()?;
        self.forward.push(current);
        Some(target)
    }

    pub fn forward(&mut self, current: T) -> Option<T> {
        let target = self.forward.pop()?;
        self.back.push(current);
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::NavigationHistory;

    #[test]
    fn goes_back_and_forward_through_jumps() {
        let mut history = NavigationHistory::default();
        assert_eq!(history.back("a:1"), None);
        history.record("a:1");
        history.record("b:5");
        assert_eq!(history.back("c:9"), Some("b:5"));
        assert_eq!(history.back("b:5"), Some("a:1"));
        assert_eq!(history.back("a:1"), None);
        assert_eq!(history.forward("a:1"), Some("b:5"));
        assert_eq!(history.forward("b:5"), Some("c:9"));
        assert_eq!(history.forward("c:9"), None);
    }

    #[test]
    fn a_new_jump_forgets_the_places_ahead() {
        let mut history = NavigationHistory::default();
        history.record("a:1");
        assert_eq!(history.back("b:2"), Some("a:1"));
        history.record("a:1");
        assert_eq!(history.forward("x:1"), None);
        assert_eq!(history.back("d:4"), Some("a:1"));
    }

    #[test]
    fn keeps_one_entry_for_repeated_jumps_from_the_same_place() {
        let mut history = NavigationHistory::default();
        history.record(1);
        history.record(1);
        assert_eq!(history.back(2), Some(1));
        assert_eq!(history.back(1), None);
    }

    #[test]
    fn forgets_the_oldest_places_past_the_limit() {
        let mut history = NavigationHistory::default();
        for place in 0..150 {
            history.record(place);
        }
        let mut current = 150;
        let mut steps = 0;
        while let Some(target) = history.back(current) {
            current = target;
            steps += 1;
        }
        assert_eq!(steps, 100);
        assert_eq!(current, 50);
    }
}

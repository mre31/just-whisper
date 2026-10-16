use crate::config::Config;
use std::time::{Duration, Instant};

/// Own the model so expiry drops the backend and releases its memory.
pub struct Resident<T> {
    pub model: Option<T>,
    idle_since: Option<Instant>,
}

impl<T> Default for Resident<T> {
    fn default() -> Self {
        Self {
            model: None,
            idle_since: None,
        }
    }
}

impl<T> Resident<T> {
    pub fn insert(&mut self, model: T, now: Instant) {
        self.model = Some(model);
        self.idle_since = Some(now);
    }

    pub fn finish_use(&mut self, now: Instant) {
        self.idle_since = Some(now);
    }

    pub fn clear(&mut self) {
        self.model = None;
        self.idle_since = None;
    }

    pub fn expire(&mut self, config: &Config, now: Instant, busy: bool) -> bool {
        if !busy
            && config.load_on_press
            && self.model.is_some()
            && self.idle_since.is_some_and(|since| {
                now.saturating_duration_since(since)
                    >= Duration::from_secs(config.model_keep_alive_secs)
            })
        {
            self.clear();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    struct Resource(Rc<Cell<usize>>);
    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn reuse_renews_deadline_and_expiry_releases_the_resource() {
        let drops = Rc::new(Cell::new(0));
        let mut resident = Resident::default();
        let config = Config::default();
        let now = Instant::now();
        resident.insert(Resource(drops.clone()), now);
        assert!(!resident.expire(&config, now + Duration::from_secs(299), false));
        resident.finish_use(now + Duration::from_secs(299));
        assert!(!resident.expire(&config, now + Duration::from_secs(300), false));
        assert!(resident.expire(&config, now + Duration::from_secs(599), false));
        assert_eq!(drops.get(), 1);
        assert!(resident.model.is_none());
        assert!(!resident.expire(&config, now + Duration::from_secs(600), false));
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn in_use_and_always_loaded_models_do_not_expire() {
        let mut resident = Resident::default();
        let mut config = Config::default();
        let now = Instant::now();
        resident.insert(42, now);
        let later = now + Duration::from_secs(3600);
        assert!(!resident.expire(&config, later, true));
        resident.finish_use(later);
        assert!(!resident.expire(&config, later, false));
        config.load_on_press = false;
        assert!(!resident.expire(&config, later + Duration::from_secs(3600), false));
        assert_eq!(resident.model, Some(42));
        config.load_on_press = true;
        assert!(resident.expire(&config, later + Duration::from_secs(3600), false));
    }

    #[test]
    fn duration_changes_apply_to_existing_models_and_zero_releases_immediately() {
        let mut resident = Resident::default();
        let now = Instant::now();
        resident.insert(42, now);
        let mut config = Config {
            model_keep_alive_secs: 60,
            ..Config::default()
        };
        assert!(resident.expire(&config, now + Duration::from_secs(60), false));
        resident.insert(42, now);
        config.model_keep_alive_secs = 0;
        assert!(!resident.expire(&config, now, true));
        assert!(resident.expire(&config, now, false));
    }
}

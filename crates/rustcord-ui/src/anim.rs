use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

const SETTLED: f32 = 0.01;

#[derive(Default)]
struct Store {
    values: HashMap<u64, f32>,
    moving: bool,
    was_moving: bool,
}

thread_local! {
    static STORE: RefCell<Store> = RefCell::new(Store::default());
}

fn key(id: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

pub fn step(value: f32, target: f32, tau: f32, dt: f32) -> f32 {
    if tau <= 0.0 {
        return target;
    }
    let next = value + (target - value) * (1.0 - (-dt.max(0.0) / tau).exp());
    if (target - next).abs() < SETTLED { target } else { next }
}

pub fn begin_frame() {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        store.was_moving = store.moving;
        store.moving = false;
    });
}

pub fn is_moving() -> bool {
    STORE.with(|store| {
        let store = store.borrow();
        store.moving || store.was_moving
    })
}

pub fn toward(ui: &imgui::Ui, id: impl Hash, target: f32, tau: f32) -> f32 {
    let dt = ui.io().delta_time;
    let id = key(id);
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        let value = *store.values.entry(id).or_insert(target);
        let next = step(value, target, tau, dt);
        if next != target {
            store.moving = true;
        }
        store.values.insert(id, next);
        next
    })
}

#[cfg(test)]
mod tests {
    use super::step;

    #[test]
    fn a_value_eases_toward_its_target_and_settles() {
        let mut value = 0.0;
        let mut frames = 0;
        while value != 40.0 {
            value = step(value, 40.0, 0.06, 1.0 / 60.0);
            frames += 1;
            assert!(frames < 120, "never settled");
        }
        assert!((15..60).contains(&frames), "settled in {frames} frames");
    }

    #[test]
    fn the_first_steps_move_most_of_the_way() {
        let after_one = step(0.0, 1.0, 0.05, 1.0 / 60.0);
        assert!(after_one > 0.2 && after_one < 0.4, "{after_one}");
    }

    #[test]
    fn a_zero_time_constant_snaps() {
        assert_eq!(step(3.0, 7.0, 0.0, 0.016), 7.0);
    }
}

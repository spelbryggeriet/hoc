use crate::{log, prelude::*};

pub fn run() {
    let mut rng = <rand_chacha::ChaCha8Rng as rand::SeedableRng>::seed_from_u64(2);
    let mut progresses = Vec::<(_, i32)>::new();

    for i in 0.. {
        let d = progresses.len();

        if rand::Rng::gen_ratio(&mut rng, 1, 5) {
            let ttl = if rand::Rng::gen_ratio(&mut rng, 1, 20) {
                rand::Rng::gen_range(&mut rng, 50..100)
            } else {
                rand::Rng::gen_range(&mut rng, 0..5)
            };

            let level = if rand::Rng::gen_ratio(&mut rng, 1, 2) {
                None
            } else if rand::Rng::gen_ratio(&mut rng, 1, 2) {
                Some(Level::Trace)
            } else if rand::Rng::gen_ratio(&mut rng, 1, 2) {
                Some(Level::Debug)
            } else if rand::Rng::gen_ratio(&mut rng, 9, 10) {
                Some(Level::Info)
            } else if rand::Rng::gen_ratio(&mut rng, 1, 2) {
                Some(Level::Warn)
            } else {
                Some(Level::Error)
            };

            let progress = log::progress(format!("Progress {}-{i}", d + 1), level, module_path!());

            progresses.push((progress, ttl));
            progresses.iter_mut().rev().fold(0, |max, (_, ttl)| {
                if *ttl <= max {
                    *ttl = max + 1;
                }
                *ttl
            });
        } else if rand::Rng::gen_ratio(&mut rng, 1, 2) {
            log!(Level::Trace, "Trace {d}-{i}");
        } else if rand::Rng::gen_ratio(&mut rng, 1, 2) {
            debug!("Debug {d}-{i}");
        } else if rand::Rng::gen_ratio(&mut rng, 9, 10) {
            info!("Info {d}-{i}");
        } else if rand::Rng::gen_ratio(&mut rng, 1, 2) {
            warn!("Warning {d}-{i}");
        } else {
            error!("Error {d}-{i}");
        }

        progresses.retain_mut(|(_, ttl)| {
            if *ttl == 0 {
                false
            } else {
                *ttl -= 1;
                true
            }
        });

        if rand::Rng::gen_ratio(&mut rng, 3, 4) {
            std::thread::sleep(std::time::Duration::from_millis(rand::Rng::gen_range(
                &mut rng,
                5..50,
            )));
        } else {
            std::thread::sleep(std::time::Duration::from_millis(rand::Rng::gen_range(
                &mut rng,
                100..1000,
            )));
        }
    }
}

pub mod manager;
pub mod render;
pub mod window;

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::config::Config;

pub struct FenceApp {
    config: Arc<Mutex<Config>>,
    desktop: PathBuf,
    config_path: PathBuf,
}

impl FenceApp {
    pub fn new(
        config: Arc<Mutex<Config>>,
        desktop: PathBuf,
        config_path: PathBuf,
    ) -> Self {
        Self {
            config,
            desktop,
            config_path,
        }
    }

    pub fn run(self) {
        let manager = manager::FenceManager::new(self.config, self.desktop, self.config_path);
        manager.run();
    }
}

use std::io;
use std::sync::{Arc, Mutex};
use crate::object::config::ObjectConfig;

pub mod config;

pub struct Object {
    config: Arc<Mutex<ObjectConfig>>,
}

impl Object {
    pub fn new(config: Arc<Mutex<ObjectConfig>>) -> Self {
        Self { config }
    }
    
    pub fn start(&self) {
        let config = self.config.clone();
        
    }
}
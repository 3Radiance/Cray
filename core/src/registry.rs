use crate::api::Factory;
use std::collections::HashMap;

#[derive(Default)]
pub struct Registry(HashMap<&'static str, Factory>);

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: &'static str, f: Factory) -> &mut Self {
        if self.0.insert(name, f).is_some() {
            panic!("duplicate layer name '{name}'");
        }
        self
    }

    pub fn get(&self, name: &str) -> Option<Factory> {
        self.0.get(name).copied()
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.0.keys().copied().collect();
        v.sort_unstable();
        v
    }
}

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub id: TypeId,
    pub name: &'static str,
}

pub fn key<T: 'static>() -> Key {
    Key {
        id: TypeId::of::<T>(),
        name: std::any::type_name::<T>(),
    }
}

#[derive(Clone, Default)]
pub struct Cx {
    items: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl Cx {
    pub fn put<T: Any + Send + Sync>(&mut self, v: T) {
        self.items.insert(TypeId::of::<T>(), Arc::new(v));
    }

    pub fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.items.get(&TypeId::of::<T>())?.downcast_ref::<T>()
    }
}

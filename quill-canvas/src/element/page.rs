//! Page entities.
//!
//! Pages are currently placeholders. They give the scene a stable entity type
//! before layout, pagination, and page-level configuration are fleshed out.

use bevy_ecs::prelude::*;

/// Configuration used when creating a page.
#[derive(Clone, Copy, Debug, Default)]
pub struct PageConfig {
    parent: u32,
}

/// Page component attached to page entities.
#[derive(Component, Clone, Copy, Debug)]
pub struct Page {
    /// Original creation config for this page.
    pub config: PageConfig,
}

impl Page {
    /// Create a page component from its config.
    pub fn new(config: PageConfig) -> Self {
        Self { config }
    }
}

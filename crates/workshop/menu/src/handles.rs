//! The menu subsystem's state handles: the chat model catalog channel
//! and the workbench bus, bundled for registration into the subsystem
//! registry so the composition root fetches them by slot instead of
//! holding them by name.

use std::sync::Arc;

use workshop_registry::{
    CatalogSink, CatalogSinkAdapter, MenuSink, MenuSinkAdapter, Registration, Registry,
};

use crate::{CatalogBus, MenuBus};

/// The menu subsystem's state handles: the chat model catalog channel
/// and the workbench bus, bundled for registration into the subsystem
/// registry so the composition root fetches them by slot instead of
/// holding them by name.
#[derive(Debug, Clone)]
pub struct MenuHandles {
    catalog: CatalogBus,
    menu: MenuBus,
}

impl MenuHandles {
    /// Bundles the catalog channel and the menu bus for registration.
    #[must_use]
    pub fn new(catalog: CatalogBus, menu: MenuBus) -> Self {
        Self { catalog, menu }
    }

    /// The chat model catalog channel.
    #[must_use]
    pub fn catalog(&self) -> &CatalogBus {
        &self.catalog
    }

    /// The workbench menu bus.
    #[must_use]
    pub fn menu(&self) -> &MenuBus {
        &self.menu
    }
}

/// Registers the menu subsystem into the registry: the catalog
/// channel's receiving end and the workbench mutators, which same-tier
/// subsystems (the gateway heartbeat's refreshes) drive through the
/// registry's push facade, plus the subsystem's state handles. The
/// returned guards keep the registrations alive; the composition root
/// holds them for the process lifetime.
pub fn register(
    registry: &Registry,
    catalog: &CatalogBus,
    menu: &MenuBus,
) -> (Registration, Registration, Registration) {
    let catalog_guard =
        registry.register_sink::<dyn CatalogSink>(Arc::new(CatalogSinkAdapter::new({
            let catalog = catalog.clone();
            move |models| catalog.publish(models)
        })));
    let menu_guard = registry.register_sink::<dyn MenuSink>(Arc::new(MenuSinkAdapter::new(
        {
            let menu = menu.clone();
            move |reachable| menu.set_gateway_reachable(reachable)
        },
        {
            let menu = menu.clone();
            move |profiles, active| menu.set_profiles(profiles, active)
        },
        {
            let menu = menu.clone();
            move || menu.restore_selection()
        },
        {
            let menu = menu.clone();
            move || menu.reconcile_catalog()
        },
    )));
    let state = registry
        .register_state::<MenuHandles>(Arc::new(MenuHandles::new(catalog.clone(), menu.clone())));
    (catalog_guard, menu_guard, state)
}

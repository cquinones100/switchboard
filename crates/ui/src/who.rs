//! Who someone is to the services a view draws people from, which an
//! avatar's photo is known by: taken from src/photos, which looks the
//! photo up, with the Markdown that places it.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Service {
    Slack,
    Linear,
    GitHub,
    /// Someone known only by their email address.
    Address,
}

/// Someone, by their id in a service: a Slack member ID (`U…`), a Linear
/// user's id, a GitHub login.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Who {
    pub service: Service,
    pub id: String,
}

impl Who {
    pub fn new(service: Service, id: &str) -> Who {
        Who {
            service,
            id: id.to_string(),
        }
    }

    /// Someone by their email address, as an invitation names them.
    pub fn address(address: &str) -> Who {
        Who::new(Service::Address, &address.trim().to_lowercase())
    }
}

use rmux_core::events::SubscriptionLimits;

use crate::signals::SignalWatcher;
#[cfg(unix)]
use crate::unix_socket::SocketFileIdentity;
#[cfg(unix)]
use crate::unix_socket_access::UnixSocketAccessController;
use crate::ConfigLoadOptions;

pub(crate) struct ServeOptions {
    pub(crate) server_signals: Option<SignalWatcher>,
    pub(crate) config_load: ConfigLoadOptions,
    pub(crate) subscription_limits: SubscriptionLimits,
    pub(crate) owner_uid: u32,
    #[cfg(unix)]
    pub(crate) socket_identity: Option<SocketFileIdentity>,
    #[cfg(unix)]
    pub(crate) socket_access: Option<UnixSocketAccessController>,
}

impl ServeOptions {
    pub(crate) fn new(
        config_load: ConfigLoadOptions,
        subscription_limits: SubscriptionLimits,
        owner_uid: u32,
    ) -> Self {
        Self {
            server_signals: None,
            config_load,
            subscription_limits,
            owner_uid,
            #[cfg(unix)]
            socket_identity: None,
            #[cfg(unix)]
            socket_access: None,
        }
    }


    #[cfg(unix)]
    pub(crate) fn with_socket_identity(
        mut self,
        socket_identity: Option<SocketFileIdentity>,
    ) -> Self {
        self.socket_identity = socket_identity;
        self
    }

    #[cfg(unix)]
    pub(crate) fn with_socket_access(mut self, socket_access: UnixSocketAccessController) -> Self {
        self.socket_access = Some(socket_access);
        self
    }

    #[cfg(unix)]
    pub(crate) fn with_server_signals(mut self, server_signals: SignalWatcher) -> Self {
        self.server_signals = Some(server_signals);
        self
    }
}

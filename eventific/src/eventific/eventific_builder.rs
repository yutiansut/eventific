use futures::Future;
use crate::Eventific;
use crate::store::{Store, MemoryStore};
use slog::Logger;
use crate::aggregate::{StateBuilder, noop_builder};
use crate::eventific::EventificError;
use std::fmt::Debug;
use crate::notification::{Sender, Listener, create_memory_notification_pair, MemorySender, MemoryListener};
use std::sync::Arc;
use colored::*;
use strum::IntoEnumIterator;
use crate::eventific::start_web_server::start_web_server;
use serde::private::de::IdentifierDeserializer;
use std::net::SocketAddr;
use std::str::FromStr;

pub struct EventificBuilder<S, D: 'static + Send + Sync + Debug, St: Store<D>, Se: Sender, L: Listener> {
    store: St,
    state_builder: StateBuilder<S, D>,
    service_name: String,
    sender: Se,
    listener: L,
    logger: Logger,
    #[cfg(feature = "playground")]
    playground: bool,
    #[cfg(feature = "with_grpc")]
    grpc_services: Vec<Box<dyn Fn(Eventific<S, D, St>) -> grpc::rt::ServerServiceDefinition + Send>>,
    #[cfg(feature = "with_grpc")]
    grpc_addr_value: String,
    web_socket: String,
    web_port: u16,
    enable_web_server: bool
}

impl<S, D: 'static + Send + Sync + Debug + Clone> EventificBuilder<S, D, MemoryStore<D>, MemorySender, MemoryListener> {
    pub fn new() -> Self {
        let logger = Logger::root(
            slog::Discard,
            o!(),
        );

        let (sender, listener) = create_memory_notification_pair();

        Self {
            store: MemoryStore::new(),
            state_builder: noop_builder,
            service_name: "default".to_owned(),
            sender,
            listener,
            logger,
            #[cfg(feature = "playground")]
            playground: false,
            #[cfg(feature = "with_grpc")]
            grpc_services: Vec::new(),
            #[cfg(feature = "with_grpc")]
            grpc_addr_value: "localhost:50051".to_owned(),
            web_socket: "127.0.0.1".to_owned(),
            web_port: 9000,
            enable_web_server: true
        }
    }
}

impl<S: 'static + Default, D: 'static + Send + Sync + Debug + Clone + AsRef<str> + IntoEnumIterator<Iterator=DI>, DI: Iterator<Item=D>, St: Store<D>, Se: 'static + Sender, L: 'static + Listener> EventificBuilder<S, D, St, Se, L> {

    pub fn logger(mut self, logger: &Logger) -> Self {
        self.logger = logger.clone();
        self
    }

    pub fn service_name(mut self, service_name: &str) -> Self {
        self.service_name = service_name.to_owned();
        self
    }

    pub fn web_socket(mut self, web_socket: &str) -> Self {
        self.web_socket = web_socket.to_owned();
        self
    }

    pub fn web_port(mut self, web_port: u16) -> Self {
        self.web_port = web_port;
        self
    }

    pub fn enable_web_server(mut self, enable_web_server: bool) -> Self {
        self.enable_web_server = enable_web_server;
        self
    }

    pub fn state_builder(mut self, state_builder: StateBuilder<S, D>) -> Self {
        self.state_builder = state_builder;
        self
    }

    pub fn store<NSt: Store<D>>(self, store: NSt) -> EventificBuilder<S, D, NSt, Se, L> {

        #[cfg(feature = "with_grpc")]
        {
            if !self.grpc_services.is_empty() {
                panic!("You can only add command handlers AFTER you have changed the store")
            }
        }

        EventificBuilder {
            store,
            state_builder: self.state_builder,
            service_name: self.service_name,
            sender: self.sender,
            listener: self.listener,
            logger: self.logger,
            #[cfg(feature = "playground")]
            playground: self.playground,
            #[cfg(feature = "with_grpc")]
            grpc_services: Vec::new(),
            #[cfg(feature = "with_grpc")]
            grpc_addr_value: self.grpc_addr_value,
            web_socket: self.web_socket,
            web_port: self.web_port,
            enable_web_server: self.enable_web_server
        }
    }

    pub fn sender<NSe: Sender>(self, sender: NSe) -> EventificBuilder<S, D, St, NSe, L> {
        EventificBuilder {
            store: self.store,
            state_builder: self.state_builder,
            service_name: self.service_name,
            sender,
            listener: self.listener,
            logger: self.logger,
            #[cfg(feature = "playground")]
            playground: self.playground,
            #[cfg(feature = "with_grpc")]
            grpc_services: self.grpc_services,
            #[cfg(feature = "with_grpc")]
            grpc_addr_value: self.grpc_addr_value,
            web_socket: self.web_socket,
            web_port: self.web_port,
            enable_web_server: self.enable_web_server
        }
    }

    pub fn listener<NL: Listener>(self, listener: NL) -> EventificBuilder<S, D, St, Se, NL> {
        EventificBuilder {
            store: self.store,
            state_builder: self.state_builder,
            service_name: self.service_name,
            sender: self.sender,
            listener,
            logger: self.logger,
            #[cfg(feature = "playground")]
            playground: self.playground,
            #[cfg(feature = "with_grpc")]
            grpc_services: self.grpc_services,
            #[cfg(feature = "with_grpc")]
            grpc_addr_value: self.grpc_addr_value,
            web_socket: self.web_socket,
            web_port: self.web_port,
            enable_web_server: self.enable_web_server
        }
    }

    #[cfg(feature = "with_grpc")]
    pub fn with_grpc_service<
        HC: 'static + Send + Fn(Eventific<S, D, St>) -> grpc::rt::ServerServiceDefinition
    >(
        mut self,
        service_callback: HC
    ) -> Self {
        self.grpc_services.push(Box::new(service_callback));
        self
    }

    #[cfg(feature = "with_grpc")]
    pub fn grpc_addr(mut self, addr: &str) -> Self {
        self.grpc_addr_value = addr.to_owned();
        self
    }

    #[cfg(feature = "playground")]
    pub fn enable_playground(mut self) -> Self {
        self.playground = true;
        self
    }

    pub fn start(self) -> impl Future<Item = Eventific<S, D, St>, Error = EventificError<D>> {
        let mut store = self.store;
        let state_builder = self.state_builder;
        let mut sender = self.sender;
        let mut listener = self.listener;
        let service_name = self.service_name;
        #[cfg(feature = "playground")]
        let use_playground = self.playground;
        #[cfg(feature = "with_grpc")]
        let grpc_command_handlers = self.grpc_services;
        #[cfg(feature = "with_grpc")]
        let grpc_addr = self.grpc_addr_value;
        let logger = self.logger.new(o!("service_name" => service_name.to_owned()));
        let web_socket = self.web_socket;
        let web_port = self.web_port;
        let enable_web_server = self.enable_web_server;

        print!("{}", "

    $$$$$$$$\\                             $$\\     $$\\  $$$$$$\\  $$\\
    $$  _____|                            $$ |    \\__|$$  __$$\\ \\__|
    $$ |  $$\\    $$\\  $$$$$$\\  $$$$$$$\\ $$$$$$\\   $$\\ $$ /  \\__|$$\\  $$$$$$$\\
    $$$$$\\\\$$\\  $$  |$$  __$$\\ $$  __$$\\\\_$$  _|  $$ |$$$$\\     $$ |$$  _____|
    $$  __|\\$$\\$$  / $$$$$$$$ |$$ |  $$ | $$ |    $$ |$$  _|    $$ |$$ /
    $$ |    \\$$$  /  $$   ____|$$ |  $$ | $$ |$$\\ $$ |$$ |      $$ |$$ |
    $$$$$$$$\\\\$  /   \\$$$$$$$\\ $$ |  $$ | \\$$$$  |$$ |$$ |      $$ |\\$$$$$$$\\
    \\________|\\_/     \\_______|\\__|  \\__|  \\____/ \\__|\\__|      \\__| \\_______|



".green());

        info!(logger, "🚀  Starting Eventific");


        store.init(&logger.clone(), &service_name)
            .map_err(EventificError::StoreInitError)
            .and_then(move |_| {
                sender.init(&logger.clone(), &service_name)
                    .map_err(EventificError::SendNotificationInitError)
                    .and_then(move |_| {
                        listener.init(&logger.clone(), &service_name)
                            .map_err(EventificError::SendNotificationInitError)
                            .and_then(move |_| {
                                let eventific = Eventific::create(logger.clone(), store, state_builder, Arc::new(sender), Arc::new(listener));

                                #[cfg(feature = "playground")]
                                {
                                    if use_playground {
                                        tokio::spawn(crate::playground::start_playground_server(&logger, &eventific));
                                    }
                                }

                                #[cfg(feature = "with_grpc")]
                                {
                                    if !grpc_command_handlers.is_empty() {
                                        crate::grpc::start_grpc_server(&logger, eventific.clone(), &grpc_addr, grpc_command_handlers)?;
                                    }
                                }

                                if enable_web_server {
                                    tokio::spawn(start_web_server(&logger, &SocketAddr::from_str(&format!("{}:{}", web_socket, web_port)).expect("Provided socket or port is not valid!")));
                                } else {
                                    info!(logger, "Web server disabled");
                                }


                                info!(logger, "Available events are:");
                                info!(logger, "");
                                for event in D::iter() {
                                    info!(logger, "{}", event.as_ref());
                                }
                                info!(logger, "");

                                info!(logger, "🤩  All setup and ready");


                                Ok(eventific)
                            })
                    })
            })
    }
}

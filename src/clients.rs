use std::{
    collections::BTreeMap,
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::UnixListener,
    },
};

use crate::{
    io_uring::{model::IoUringAction, ring::AcceptCompleted},
    state::{AnyAction, Dispatcher, Model},
};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ClientId(u64);

pub struct Clients {
    current_id: u64,
    fd: UnixListener,
    map: BTreeMap<ClientId, Client>,
}

#[derive(Debug)]
pub enum ClientsAction {
    StartListen,
    NewClient { new_client: RawFd, fd: RawFd },
}

impl Clients {
    fn next_id(&mut self) -> ClientId {
        let id = self.current_id;
        self.current_id += 1;
        ClientId(id)
    }

    fn on_accept(result: std::io::Result<AcceptCompleted>) -> AnyAction {
        let AcceptCompleted { fd, new_client } = result.unwrap();
        AnyAction::from(ClientsAction::NewClient { fd, new_client })
    }
}

impl Model for Clients {
    type InitParams = ();
    type Action = ClientsAction;

    fn init(params: Self::InitParams) -> Box<Self> {
        let unix = UnixListener::bind("/var/run/user/1000/wayland-1").unwrap();
        Box::new(Self {
            current_id: 0,
            fd: unix,
            map: BTreeMap::default(),
        })
    }

    fn reducer(&mut self, action: &ClientsAction) -> Result<(), ()> {
        // dbg!(action);
        match action {
            ClientsAction::StartListen => {}
            ClientsAction::NewClient { fd, new_client } => {
                let client_id = self.next_id();
                log::trace!("new_client id={:?} fd={:?}", client_id, new_client);
                self.map.insert(client_id, Client { fd: *new_client });
            }
        }
        Ok(())
    }

    fn effect(&self, action: &ClientsAction, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        match action {
            ClientsAction::StartListen => {
                dispatcher.dispatch(IoUringAction::Accept {
                    fd: self.fd.as_raw_fd(),
                    dispatch_to: Self::on_accept,
                });
            }
            ClientsAction::NewClient { fd, new_client } => {
                dispatcher.dispatch(IoUringAction::Accept {
                    fd: self.fd.as_raw_fd(),
                    dispatch_to: Self::on_accept,
                });
            }
        }
        Ok(())
    }
}

struct Client {
    fd: RawFd,
}

pub enum ClientAction {
    StartReceive,
    B,
}

impl Model for Client {
    // type State = FrontendState;
    type InitParams = ();
    type Action = ClientAction;

    fn init(params: Self::InitParams) -> Box<Self> {
        todo!()
    }

    fn reducer(&mut self, action: &ClientAction) -> Result<(), ()> {
        match action {
            ClientAction::StartReceive => todo!(),
            ClientAction::B => todo!(),
        }
    }

    fn effect(
        &self,
        action: &ClientAction,
        // state: &Self::State,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ()> {
        todo!()
    }
}

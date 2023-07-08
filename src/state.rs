use std::{
    any::{Any, TypeId},
    collections::{HashMap, VecDeque},
};

use crate::io_uring::model::IoUringAction;

type Result<T> = std::result::Result<T, ()>;

pub struct Dispatcher<'a> {
    queue: &'a mut VecDeque<AnyAction>,
}

impl Dispatcher<'_> {
    pub fn dispatch<A>(&mut self, action: A)
    where
        A: Sized + 'static,
    {
        self.queue.push_back(AnyAction {
            id: std::any::TypeId::of::<A>(),
            type_name: std::any::type_name::<A>(),
            ptr: Box::new(action),
        });
    }

    pub fn dispatch_any(&mut self, action: AnyAction) {
        self.queue.push_back(action);
    }
}

// pub trait Redispatcher<Result> {
//     fn into_action(result: Result) -> AnyAction;
// }

pub type Redispatcher<R> = fn(R) -> AnyAction;

type ActionId = std::any::TypeId;

pub struct Runner {
    models: HashMap<ActionId, Vec<AnyModel>>,
    queue: VecDeque<AnyAction>,
}

impl Runner {
    pub fn new() -> Self {
        Self {
            models: HashMap::with_capacity(128),
            queue: VecDeque::with_capacity(1024),
        }
    }

    pub fn add_model<M>(&mut self, model: Box<M>)
    where
        M: Model,
    {
        let models = self
            .models
            .entry(std::any::TypeId::of::<M::Action>())
            .or_default();

        models.push(model.into_vtable());
    }

    pub fn run(&mut self) {
        loop {
            while let Some(action) = self.queue.pop_front() {
                self.dispatch_action(action)
            }
            self.dispatch(IoUringAction::WaitForCompletion);
        }
    }

    pub fn dispatch<A>(&mut self, action: A)
    where
        A: Sized + 'static,
    {
        self.queue.push_back(AnyAction {
            id: std::any::TypeId::of::<A>(),
            type_name: std::any::type_name::<A>(),
            ptr: Box::new(action),
        });
    }

    fn dispatch_action(&mut self, action: AnyAction) {
        assert_ne!(action.id, std::any::TypeId::of::<AnyAction>());

        let Some(models) = self.models.get_mut(&action.action_id()) else {
            eprintln!("action not found1 {}", action.type_name);
            return;
        };

        let mut dispatcher = Dispatcher {
            queue: &mut self.queue,
        };

        for model in models {
            (model.vtable.try_call_reducer_effect)(&mut model.state, &action, &mut dispatcher)
                .unwrap();
        }
    }
}

pub struct AnyAction {
    id: TypeId,
    type_name: &'static str,
    ptr: Box<dyn Any>,
}

impl AnyAction {
    fn action_id(&self) -> ActionId {
        self.id
    }

    // pub fn empty() -> Self {
    //     Self {
    //         id: std::any::TypeId::of::<()>(),
    //         ptr: Box::new(()),
    //     }
    // }

    pub fn from<T: 'static>(v: T) -> Self {
        Self {
            id: std::any::TypeId::of::<T>(),
            ptr: Box::new(v),
            type_name: std::any::type_name::<T>(),
        }
    }
}

pub struct AnyModel {
    state: Box<dyn Any>,
    vtable: ModelVTable,
}

struct ModelVTable {
    try_call_reducer_effect:
        fn(state: &mut Box<dyn Any>, action: &AnyAction, dispatcher: &mut Dispatcher) -> Result<()>,
}

pub trait PrivateModel
where
    Self: Sized + 'static,
{
    type Action: 'static;

    fn into_vtable(self: Box<Self>) -> AnyModel;
    fn try_call_reducer_effect(
        state: &mut Box<dyn Any>,
        action: &AnyAction,
        dispatcher: &mut Dispatcher,
    ) -> Result<()>;
}

impl<T> PrivateModel for T
where
    T: Model,
{
    type Action = <T as Model>::Action;

    fn into_vtable(self: Box<Self>) -> AnyModel {
        let state = self;
        let vtable = ModelVTable {
            try_call_reducer_effect: Self::try_call_reducer_effect,
        };
        AnyModel { state, vtable }
    }

    fn try_call_reducer_effect(
        state: &mut Box<dyn Any>,
        action: &AnyAction,
        dispatcher: &mut Dispatcher,
    ) -> Result<()> {
        let Some(state) = state.downcast_mut::<Self>() else {
            eprintln!("state not found");
            return Ok(());
        };
        let Some(action) = action.ptr.downcast_ref::<Self::Action>() else {
            eprintln!("action not found2");
            return Ok(());
        };
        state.reducer(action)?;
        state.effect(action, dispatcher)?;
        Ok(())
    }
}

pub trait Model
where
    Self: Sized + 'static,
{
    type InitParams: 'static;
    type Action: Sized + 'static;

    fn init(params: Self::InitParams) -> Box<Self>;
    fn reducer(&mut self, action: &Self::Action) -> Result<()>;
    fn effect(&self, action: &Self::Action, dispatcher: &mut Dispatcher) -> Result<()>;
}

mod backend {
    use super::*;

    enum BackendAction {
        NewDrmDevice,
        NewConnector,
        NewInputDevice,
        DevicesEnumerated,
    }

    struct Backend;
    struct BackendState;

    impl Model for BackendState {
        // type State = BackendState;
        type InitParams = ();
        type Action = BackendAction;

        fn init(params: Self::InitParams) -> Box<Self> {
            todo!()
        }

        fn reducer(&mut self, action: &Self::Action) -> Result<()> {
            match action {
                Self::Action::NewDrmDevice => todo!(),
                Self::Action::NewConnector => todo!(),
                Self::Action::NewInputDevice => todo!(),
                Self::Action::DevicesEnumerated => todo!(),
            }
        }

        fn effect(
            &self,
            action: &Self::Action,
            // state: &Self::State,
            dispatcher: &mut Dispatcher,
        ) -> Result<()> {
            todo!()
        }
    }
}

mod frontend {
    use super::*;

    enum FrontendAction {
        Push,
        Click,
    }

    // struct Frontend;
    struct FrontendState;

    impl Model for FrontendState {
        // type State = FrontendState;
        type InitParams = ();
        type Action = FrontendAction;

        fn init(params: Self::InitParams) -> Box<Self> {
            todo!()
        }

        fn reducer(&mut self, action: &Self::Action) -> Result<()> {
            match action {
                Self::Action::Push => todo!(),
                Self::Action::Click => todo!(),
            }
        }

        fn effect(
            &self,
            action: &Self::Action,
            // state: &Self::State,
            dispatcher: &mut Dispatcher,
        ) -> Result<()> {
            todo!()
        }
    }
}

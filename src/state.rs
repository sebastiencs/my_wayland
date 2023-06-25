use std::{
    any::Any,
    collections::{HashMap, VecDeque},
};

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
            ptr: Box::new(action),
        });
    }
}

type ActionId = std::any::TypeId;

struct Runner {
    models: HashMap<ActionId, Vec<AnyModel>>,
    queue: VecDeque<AnyAction>,
}

impl Runner {
    fn new() -> Self {
        Self {
            models: HashMap::with_capacity(128),
            queue: VecDeque::with_capacity(1024),
        }
    }

    fn dispatch_action(&mut self, action: AnyAction) {
        let Some(models) = self.models.get_mut(&action.action_id()) else {
            return;
        };

        let mut dispatcher = Dispatcher {
            queue: &mut self.queue,
        };

        for model in models {
            (model.vtable.try_call_reducer)(&mut model.state, &action).unwrap();
            (model.vtable.try_call_effect)(&action, &mut dispatcher).unwrap();
        }
    }
}

struct AnyAction {
    ptr: Box<dyn Any>,
}

impl AnyAction {
    fn action_id(&self) -> ActionId {
        self.ptr.type_id()
    }

    pub fn downcast<T: Any>(self) -> Option<Box<T>> {
        self.ptr.downcast::<T>().ok()
    }
}

struct AnyModel {
    state: Box<dyn Any>,
    vtable: ModelVTable,
}

struct ModelVTable {
    try_call_reducer: fn(state: &mut dyn Any, action: &AnyAction) -> Result<()>,
    try_call_effect: fn(action: &AnyAction, dispatcher: &mut Dispatcher) -> Result<()>,
}

trait PrivateModel<Action>
where
    Self: Sized + 'static,
{
    type InitParams: 'static;

    fn init_with_vtable(params: Self::InitParams) -> AnyModel;
    fn try_call_reducer(state: &mut dyn Any, action: &AnyAction) -> Result<()>;
    fn try_call_effect(action: &AnyAction, dispatcher: &mut Dispatcher) -> Result<()>;
}

impl<T, Action> PrivateModel<Action> for T
where
    T: Model<Action>,
    Action: Sized + 'static,
{
    type InitParams = <T as Model<Action>>::InitParams;

    fn init_with_vtable(params: Self::InitParams) -> AnyModel {
        let state = Self::init(params);
        let vtable = ModelVTable {
            try_call_reducer: Self::try_call_reducer,
            try_call_effect: Self::try_call_effect,
        };
        AnyModel { state, vtable }
    }

    fn try_call_reducer(state: &mut dyn Any, action: &AnyAction) -> Result<()> {
        let Some(state) = state.downcast_mut::<<Self as Model<Action>>::State>() else {
            return Ok(());
        };

        let Some(action) = action.ptr.downcast_ref::<Action>() else {
            return Ok(());
        };

        Self::reducer(state, action)
    }

    fn try_call_effect(action: &AnyAction, dispatcher: &mut Dispatcher) -> Result<()> {
        todo!()
    }
}

pub trait Model<Action>
where
    Self: Sized + 'static,
    Action: Sized + 'static,
{
    type State: 'static;
    type InitParams: 'static;

    fn init(params: Self::InitParams) -> Box<Self::State>;
    fn reducer(state: &mut Self::State, action: &Action) -> Result<()>;
    fn effect(action: &Action, dispatcher: &mut Dispatcher) -> Result<()>;
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

    impl Model<BackendAction> for Backend {
        type State = BackendState;
        type InitParams = ();

        fn init(params: Self::InitParams) -> Box<Self::State> {
            todo!()
        }

        fn reducer(state: &mut Self::State, action: &BackendAction) -> Result<()> {
            match action {
                BackendAction::NewDrmDevice => todo!(),
                BackendAction::NewConnector => todo!(),
                BackendAction::NewInputDevice => todo!(),
                BackendAction::DevicesEnumerated => todo!(),
            }
        }

        fn effect(action: &BackendAction, dispatcher: &mut Dispatcher) -> Result<()> {
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

    struct Frontend;
    struct FrontendState;

    impl Model<FrontendAction> for Frontend {
        type State = FrontendState;
        type InitParams = ();

        fn init(params: Self::InitParams) -> Box<Self::State> {
            todo!()
        }

        fn reducer(state: &mut Self::State, action: &FrontendAction) -> Result<()> {
            match action {
                FrontendAction::Push => todo!(),
                FrontendAction::Click => todo!(),
            }
        }

        fn effect(action: &FrontendAction, dispatcher: &mut Dispatcher) -> Result<()> {
            todo!()
        }
    }
}

use std::{
    any::{Any, TypeId},
    collections::{BTreeMap, VecDeque},
};

use crate::io_uring::model::IoUringAction;

type Result<T> = std::result::Result<T, ()>;

pub struct Dispatcher<'a> {
    queue: &'a mut VecDeque<AnyAction>,
    pending_models: BTreeMap<ActionId, Vec<AnyModel>>,
}

impl Dispatcher<'_> {
    pub fn dispatch<A>(&mut self, action: A)
    where
        A: Sized + 'static,
    {
        assert_ne!(TypeId::of::<A>(), TypeId::of::<AnyAction>());
        self.queue.push_back(AnyAction::from(action));
    }

    pub fn dispatch_to<A, I>(&mut self, id: I, action: A)
    where
        I: Into<u64>,
        A: Sized + 'static,
    {
        assert_ne!(TypeId::of::<A>(), TypeId::of::<AnyAction>());
        self.queue.push_back(AnyAction::from_id(id, action));
    }

    pub fn dispatch_any(&mut self, action: AnyAction) {
        self.queue.push_back(action);
    }

    pub fn add_model_with_id<I, M>(&mut self, id: I, model: M)
    where
        I: Into<u64>,
        M: Model,
    {
        let action_id = ActionId::of_with::<M::Action, _>(id);
        let models = self.pending_models.entry(action_id).or_default();
        models.push(Box::new(model).into_vtable());
    }

    pub fn start_model_with_id<I, M>(&mut self, id: I, model: M)
    where
        I: Into<u64> + Copy,
        M: Model,
    {
        self.add_model_with_id(id, model);
        self.dispatch_to(id, M::START.unwrap());
    }
}

pub enum Redispatcher<R> {
    Fun(fn(R) -> AnyAction),
    WithId((u64, fn(R) -> AnyAction)),
}

impl<R: std::fmt::Debug> std::fmt::Debug for Redispatcher<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fun(_fun) => f.write_str("ptr"),
            Self::WithId((id, _fun)) => f.write_fmt(format_args!("id={:?}", id)),
        }
    }
}

impl<R> Clone for Redispatcher<R> {
    fn clone(&self) -> Self {
        match self {
            Self::Fun(arg0) => Self::Fun(*arg0),
            Self::WithId(arg0) => Self::WithId(*arg0),
        }
    }
}

impl<R> Copy for Redispatcher<R> {}

impl<R> Redispatcher<R> {
    pub fn new(fun: fn(R) -> AnyAction) -> Self {
        Self::Fun(fun)
    }

    pub fn new_with_id<V>(id: V, fun: fn(R) -> AnyAction) -> Self
    where
        V: Into<u64>,
    {
        Self::WithId((id.into(), fun))
    }
}

impl<R> Redispatcher<R> {
    pub fn make(self, value: R) -> AnyAction {
        match self {
            Self::Fun(fun) => fun(value),
            Self::WithId((id, fun)) => {
                let mut action = fun(value);
                action.id.specifier = Some(id);
                action
            }
        }
    }
}

// pub type Redispatcher<R> = fn(R) -> AnyAction;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ActionId {
    type_id: TypeId,
    specifier: Option<u64>,
}

impl ActionId {
    pub fn of<T>() -> Self
    where
        T: 'static,
    {
        Self {
            type_id: TypeId::of::<T>(),
            specifier: None,
        }
    }

    pub fn of_with<T, S>(spec: S) -> Self
    where
        T: 'static,
        S: Into<u64>,
    {
        Self {
            type_id: TypeId::of::<T>(),
            specifier: Some(spec.into()),
        }
    }
}

pub struct Runner {
    models: BTreeMap<ActionId, Vec<AnyModel>>,
    queue: VecDeque<AnyAction>,
}

impl Runner {
    pub fn new() -> Self {
        Self {
            models: BTreeMap::default(),
            queue: VecDeque::with_capacity(1024),
        }
    }

    pub fn add_model<M>(&mut self, model: Box<M>)
    where
        M: Model,
    {
        let models = self.models.entry(ActionId::of::<M::Action>()).or_default();

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
        self.queue.push_back(AnyAction::from(action));
    }

    fn dispatch_action(&mut self, action: AnyAction) {
        assert_ne!(action.id, ActionId::of::<AnyAction>());

        let Some(models) = self.models.get_mut(action.action_id()) else {
            eprintln!("action not found1 {}", action.type_name);
            return;
        };

        let mut dispatcher = Dispatcher {
            queue: &mut self.queue,
            pending_models: BTreeMap::default(),
        };

        for model in models {
            (model.vtable.try_call_reducer_effect)(&mut model.state, &action, &mut dispatcher)
                .unwrap();
        }

        for (action_id, pending_models) in dispatcher.pending_models {
            self.models
                .entry(action_id)
                .or_default()
                .extend(pending_models);
        }
    }
}

pub struct AnyAction {
    id: ActionId,
    type_name: &'static str,
    ptr: Box<dyn Any>,
}

impl AnyAction {
    fn action_id(&self) -> &ActionId {
        &self.id
    }

    pub fn empty() -> Self {
        Self {
            id: ActionId::of::<()>(),
            ptr: Box::new(()),
            type_name: std::any::type_name::<()>(),
        }
    }

    pub fn from<T: 'static>(v: T) -> Self {
        // Panic when calling `AnyAction::from(AnyAction { .. })`
        assert_ne!(TypeId::of::<T>(), TypeId::of::<AnyAction>());
        Self {
            id: ActionId::of::<T>(),
            ptr: Box::new(v),
            type_name: std::any::type_name::<T>(),
        }
    }

    pub fn from_id<T, S>(id: S, v: T) -> Self
    where
        T: 'static,
        S: Into<u64>,
    {
        // Panic when calling `AnyAction::from(AnyAction { .. })`
        assert_ne!(TypeId::of::<T>(), TypeId::of::<AnyAction>());
        Self {
            id: ActionId::of_with::<T, S>(id),
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
        state.reducer(action, dispatcher)?;
        Ok(())
    }
}

pub trait Model
where
    Self: Sized + 'static,
{
    type InitParams: 'static;
    type Action: Sized + 'static;
    const START: Option<Self::Action> = None;

    fn init(params: Self::InitParams) -> Box<Self>;
    fn reducer(&mut self, action: &Self::Action, dispatcher: &mut Dispatcher) -> Result<()>;
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

        fn reducer(&mut self, action: &Self::Action, dispatcher: &mut Dispatcher) -> Result<()> {
            match action {
                Self::Action::NewDrmDevice => todo!(),
                Self::Action::NewConnector => todo!(),
                Self::Action::NewInputDevice => todo!(),
                Self::Action::DevicesEnumerated => todo!(),
            }
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

        fn reducer(&mut self, action: &Self::Action, dispatcher: &mut Dispatcher) -> Result<()> {
            match action {
                Self::Action::Push => todo!(),
                Self::Action::Click => todo!(),
            }
        }
    }
}

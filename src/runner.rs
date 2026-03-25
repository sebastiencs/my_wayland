use std::{
    any::{Any, TypeId},
    cmp::Ordering,
    collections::{BTreeMap, VecDeque},
};

use crate::{io_uring::model::IoUringAction, utils::strip_type_name, State};

type Result<T> = std::result::Result<T, ()>;

pub struct Dispatcher {
    /// Actions to dispatch
    queue: VecDeque<AnyAction>,
    /// Models to add
    pending_add: VecDeque<(ActionId, AnyModel)>,
    /// Models to remove
    pending_remove: VecDeque<ActionId>,
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self {
            queue: VecDeque::with_capacity(1024),
            pending_add: VecDeque::with_capacity(1024),
            pending_remove: VecDeque::with_capacity(1024),
        }
    }
}

impl Dispatcher {
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

    pub fn redispatch_to<R>(&mut self, redispatcher: Redispatcher<R>, action: R) {
        if let Some(action) = redispatcher.make(action) {
            self.queue.push_back(action);
        };
    }

    pub fn start<M>(&mut self)
    where
        M: Model,
    {
        self.dispatch(M::START.unwrap());
    }

    pub fn add_model<I, M>(&mut self, id: I, model: M)
    where
        I: Into<u64>,
        M: Model,
    {
        let action_id = ActionId::of_with::<M::Action, _>(id);
        self.pending_add
            .push_back((action_id, Box::new(model).into_vtable()));
    }

    pub fn start_model<I, M>(&mut self, id: I, model: M)
    where
        I: Into<u64> + Copy,
        M: Model,
    {
        self.add_model(id, model);
        self.dispatch_to(id, M::START.unwrap());
    }

    pub fn start_model2<M>(&mut self, model: M)
    where
        M: Model,
    {
        let action_id = ActionId::of::<M::Action>();
        self.pending_add
            .push_back((action_id, Box::new(model).into_vtable()));
        self.dispatch(M::START.unwrap());
    }

    pub fn stop_model<I, M>(&mut self, id: I)
    where
        I: Into<u64> + Copy,
        M: Model,
    {
        let action_id = ActionId::of_with::<M::Action, _>(id);
        self.pending_remove.push_back(action_id);
    }
}

pub enum Redispatcher<R> {
    Fun {
        ptr: fn(R) -> AnyAction,
        type_name: &'static str,
    },
    WithId {
        ptr: fn(R) -> AnyAction,
        id: u64,
        /// For printing/debug purpose only
        /// So we know to which model we redispatch
        type_name: &'static str,
    },
    Into {
        ptr: fn(R) -> AnyAction,
        /// For printing/debug purpose only
        /// So we know to which model we redispatch
        type_name: &'static str,
    },
    None,
}

impl<R> Clone for Redispatcher<R> {
    fn clone(&self) -> Self {
        match self {
            Self::Fun { ptr, type_name } => Self::Fun { ptr: *ptr, type_name },
            Self::WithId { ptr, id, type_name } => Self::WithId { ptr: *ptr, id: *id, type_name },
            Self::Into { ptr, type_name } => Self::Into { ptr: *ptr, type_name },
            Self::None => Self::None,
        }
    }
}

impl<R: std::fmt::Debug> std::fmt::Debug for Redispatcher<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fun { ptr: _, type_name } => f.write_str(strip_type_name(type_name)),
            Self::WithId { id, ptr: _, type_name } => {
                f.write_fmt(format_args!("{}({:?})", strip_type_name(type_name), id))
            }
            Self::Into { ptr: _, type_name } => {
                f.write_fmt(format_args!("{}", strip_type_name(type_name)))
            }
            Self::None => f.write_str("None"),
        }
    }
}

impl<R> Redispatcher<R> {
    pub fn new<T: Model>(ptr: fn(R) -> AnyAction) -> Self {
        let type_name = std::any::type_name::<T>();
        Self::Fun { ptr, type_name }
    }

    pub fn new_noname(ptr: fn(R) -> AnyAction) -> Self {
        Self::Fun { ptr, type_name: "" }
    }

    pub fn into<T>() -> Self
    where
        T: From<R> + 'static,
    {
        fn auto_generated<R, T: From<R> + 'static>(result: R) -> AnyAction {
            AnyAction::from(T::from(result))
        }
        let ptr = auto_generated::<R, T>;
        let name = std::any::type_name::<T>();
        Self::Into { ptr, type_name: name }
    }

    pub fn id<V>(id: V, ptr: fn(R) -> AnyAction) -> Self
    where
        V: Into<u64>,
    {
        let id = id.into();
        let name = std::any::type_name::<V>();
        Self::WithId { id, ptr, type_name: name }
    }
}

impl<R> Redispatcher<R> {
    pub fn make(self, value: R) -> Option<AnyAction> {
        match self {
            Self::Fun { ptr, type_name: _ } => Some(ptr(value)),
            Self::Into { ptr, type_name: _ } => Some(ptr(value)),
            Self::WithId { id, ptr, type_name: _ } => {
                let mut action = ptr(value);
                action.id.specifier = Some(id);
                Some(action)
            }
            Self::None => None,
        }
    }
}

// pub type Redispatcher<R> = fn(R) -> AnyAction;

struct ActionId {
    type_id: TypeId,
    /// This is an additional information about the action's model.
    /// It could be a `PeerId`, `ClientId`, etc, or any model
    /// having different instances alive.
    /// So the action go straight to the model instance.
    specifier: Option<u64>,
    /// `std::any::type_name` of the action type `T` (for logs / `Debug`).
    /// Not used for [`PartialEq`] / [`Ord`]; identity is only `type_id` + `specifier`.
    type_name: &'static str,
}

impl PartialEq for ActionId {
    fn eq(&self, other: &Self) -> bool {
        let &ActionId { type_id, specifier, type_name: _ } = self;
        type_id == other.type_id && specifier == other.specifier
    }
}

impl Eq for ActionId {}

impl PartialOrd for ActionId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ActionId {
    fn cmp(&self, other: &Self) -> Ordering {
        let &ActionId { type_id, specifier, type_name: _ } = self;
        type_id
            .cmp(&other.type_id)
            .then_with(|| specifier.cmp(&other.specifier))
    }
}

impl std::fmt::Debug for ActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let &ActionId { type_name, specifier, type_id: _ } = self;
        f.debug_struct("ActionId")
            .field("type_name", &strip_type_name(type_name))
            .field("specifier", &specifier)
            .finish()
    }
}

impl ActionId {
    pub fn of<T>() -> Self
    where
        T: 'static,
    {
        Self {
            type_id: TypeId::of::<T>(),
            specifier: None,
            type_name: std::any::type_name::<T>(),
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
            type_name: std::any::type_name::<T>(),
        }
    }
}

pub struct Runner {
    models: BTreeMap<ActionId, AnyModel>,
    state: State,
}

impl Runner {
    pub fn with_state(state: State) -> Self {
        Self { models: BTreeMap::default(), state }
    }

    pub fn add_model<M>(&mut self, model: Box<M>)
    where
        M: Model,
    {
        self.models
            .insert(ActionId::of::<M::Action>(), model.into_vtable());
    }

    pub fn run(&mut self, mut dispatcher: Dispatcher) {
        loop {
            while let Some(action) = dispatcher.queue.pop_front() {
                self.process_action(action, &mut dispatcher)
            }
            dispatcher.dispatch(IoUringAction::WaitForCompletion);
        }
    }

    fn process_action(&mut self, action: AnyAction, dispatcher: &mut Dispatcher) {
        assert_ne!(action.id, ActionId::of::<AnyAction>());

        let Some(AnyModel { model, vtable }) = self.models.get_mut(action.action_id()) else {
            log::warn!("dropping action for removed model {}", action.type_name);
            return;
        };
        (vtable.try_process)(model, &mut self.state, action, dispatcher).unwrap();
        self.post_process(dispatcher);
    }

    fn post_process(&mut self, dispatcher: &mut Dispatcher) {
        self.maybe_add_models(dispatcher);
        self.maybe_remove_models(dispatcher);
    }

    fn maybe_add_models(&mut self, dispatcher: &mut Dispatcher) {
        // Add new models
        while let Some((action_id, model)) = dispatcher.pending_add.pop_front() {
            self.models.insert(action_id, model);
        }
    }

    fn maybe_remove_models(&mut self, dispatcher: &mut Dispatcher) {
        // Stop models
        if dispatcher.pending_remove.is_empty() {
            return;
        }
        // We use an index here because `try_stop` might add new ids to `dispatcher.pending_remove`
        let mut index = 0;
        while let Some(id) = dispatcher.pending_remove.get(index) {
            if let Some(AnyModel { model, vtable }) = self.models.get_mut(id) {
                (vtable.try_stop)(model, &mut self.state, dispatcher).unwrap();
            } else {
                log::warn!(
                    "dropping stop action for removed model {}",
                    strip_type_name(id.type_name)
                );
            }
            index += 1;
        }
        // Remove them in reverse order
        while let Some(id) = dispatcher.pending_remove.pop_back() {
            self.models.remove(&id);
        }
    }
}

pub struct AnyAction {
    id: ActionId,
    ptr: Box<dyn Any>,
    /// For printing/debug purpose only
    type_name: &'static str,
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
    model: Box<dyn Any>,
    vtable: ModelVTable,
}

struct ModelVTable {
    try_process: fn(
        model: &mut Box<dyn Any>,
        state: &mut State,
        action: AnyAction,
        dispatcher: &mut Dispatcher,
    ) -> Result<()>,
    try_stop:
        fn(model: &mut Box<dyn Any>, state: &mut State, dispatcher: &mut Dispatcher) -> Result<()>,
}

pub trait PrivateModel
where
    Self: Sized + 'static,
{
    fn into_vtable(self: Box<Self>) -> AnyModel;
    fn try_process(
        model: &mut Box<dyn Any>,
        state: &mut State,
        action: AnyAction,
        dispatcher: &mut Dispatcher,
    ) -> Result<()>;
    fn try_stop(
        model: &mut Box<dyn Any>,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) -> Result<()>;
}

impl<T> PrivateModel for T
where
    T: Model,
{
    fn into_vtable(self: Box<Self>) -> AnyModel {
        let model = self;
        let vtable = ModelVTable { try_process: Self::try_process, try_stop: Self::try_stop };
        AnyModel { model, vtable }
    }

    fn try_process(
        model: &mut Box<dyn Any>,
        state: &mut State,
        action: AnyAction,
        dispatcher: &mut Dispatcher,
    ) -> Result<()> {
        let Some(model) = model.downcast_mut::<Self>() else {
            eprintln!("model not found");
            return Ok(());
        };
        let Ok(action) = action.ptr.downcast::<T::Action>() else {
            eprintln!("action not found2");
            return Ok(());
        };
        model.process(state, *action, dispatcher)?;
        Ok(())
    }

    fn try_stop(
        model: &mut Box<dyn Any>,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) -> Result<()> {
        let Some(model) = model.downcast_mut::<Self>() else {
            panic!("model not found");
        };
        let Some(action) = Self::STOP else {
            panic!("Model::STOP is not defined for {}", std::any::type_name::<T>());
        };
        model.process(state, action, dispatcher)?;
        Ok(())
    }
}

pub trait Model
where
    Self: Sized + 'static,
{
    type Action: Sized + 'static;
    const START: Option<Self::Action> = None;
    const STOP: Option<Self::Action> = None;

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> Result<()>;
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
        type Action = BackendAction;

        fn process(
            &mut self,
            _state: &mut State,
            action: Self::Action,
            _dispatcher: &mut Dispatcher,
        ) -> Result<()> {
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
        type Action = FrontendAction;

        fn process(
            &mut self,
            _state: &mut State,
            action: Self::Action,
            _dispatcher: &mut Dispatcher,
        ) -> Result<()> {
            match action {
                Self::Action::Push => todo!(),
                Self::Action::Click => todo!(),
            }
        }
    }
}

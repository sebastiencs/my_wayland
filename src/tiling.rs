use std::collections::{BTreeMap, BTreeSet};

use crate::{
    config::DecorationInsets,
    protocol_generated::{xdg_toplevel, XdgSurfaceId},
    region::RectI,
    state::{ManagedWindowMode, ObjectId, State, WorkspaceId},
    transform::SizeI,
    wayland_objects::{
        wl_surface::{Role, WlSurface},
        xdg_surface::xdg_toplevel::{ToplevelDecorationMode, XdgToplevel},
        zwlr_layer_surface_v1::output_usable_bounds,
    },
};

pub const EDGE_TOP: u32 = 1;
pub const EDGE_BOTTOM: u32 = 2;
pub const EDGE_LEFT: u32 = 4;
pub const EDGE_RIGHT: u32 = 8;

const MIN_CHILD_WEIGHT: f64 = 0.1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GapPolicy {
    pub outer_gap: i32,
    pub inner_gap: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayoutOperation {
    surface_object_id: ObjectId,
    frame_rect: RectI,
    content_rect: RectI,
}

type DecorationInsetsMap = BTreeMap<ObjectId, DecorationInsets>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitialLayoutConfigure {
    pub frame_rect: RectI,
    pub tiled_edges: u32,
    pub fullscreen: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TiledEdgeConstraints {
    left: Option<i32>,
    right: Option<i32>,
    top: Option<i32>,
    bottom: Option<i32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LayoutSizeHint {
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayoutNodeId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

impl Default for SplitAxis {
    fn default() -> Self {
        Self::Horizontal
    }
}

impl SplitAxis {
    fn opposite(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LayoutChild {
    pub node_id: LayoutNodeId,
    pub weight: f64,
}

#[derive(Clone, Debug)]
pub struct ContainerNode {
    pub axis: SplitAxis,
    pub parent: Option<LayoutNodeId>,
    pub children: Vec<LayoutChild>,
    pub focused_child: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct LeafNode {
    pub surface_object_id: ObjectId,
    pub parent: Option<LayoutNodeId>,
    pub last_applied_rect: Option<RectI>,
    pub tiled_edges: u32,
}

#[derive(Clone, Debug)]
pub enum LayoutNode {
    Container(ContainerNode),
    Leaf(LeafNode),
}

#[derive(Clone, Debug, Default)]
pub struct FloatingState {
    pub stack: Vec<ObjectId>,
}

#[derive(Clone, Debug)]
pub struct FullscreenState {
    pub surface_object_id: ObjectId,
}

#[derive(Clone, Copy, Debug)]
pub struct TiledResizeState {
    axis: SplitAxis,
    leaf_index: usize,
    neighbor_index: usize,
    pair_weight: f64,
    initial_leaf_weight: f64,
    axis_span: i32,
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceLayout {
    pub root: Option<LayoutNodeId>,
    pub nodes: BTreeMap<LayoutNodeId, LayoutNode>,
    pub surface_leaves: BTreeMap<ObjectId, LayoutNodeId>,
    pub floating: FloatingState,
    pub fullscreen: Option<FullscreenState>,
    pub focused_leaf: Option<LayoutNodeId>,
    pub dirty: bool,
    next_node_id: u64,
}

impl WorkspaceLayout {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn contains_surface(&self, surface_object_id: ObjectId) -> bool {
        self.surface_leaves.contains_key(&surface_object_id)
            || self.floating.stack.contains(&surface_object_id)
            || self
                .fullscreen
                .as_ref()
                .is_some_and(|fullscreen| fullscreen.surface_object_id == surface_object_id)
    }

    pub fn is_tiled_surface(&self, surface_object_id: ObjectId) -> bool {
        self.surface_leaves.contains_key(&surface_object_id)
    }

    pub fn is_floating_surface(&self, surface_object_id: ObjectId) -> bool {
        self.floating.stack.contains(&surface_object_id)
    }

    pub fn tiled_rect(&self, surface_object_id: ObjectId) -> Option<RectI> {
        let leaf_id = *self.surface_leaves.get(&surface_object_id)?;
        let leaf = self.nodes.get(&leaf_id)?.as_leaf()?;
        leaf.last_applied_rect
    }

    pub fn tiled_edges(&self, surface_object_id: ObjectId) -> Option<u32> {
        let leaf_id = *self.surface_leaves.get(&surface_object_id)?;
        let leaf = self.nodes.get(&leaf_id)?.as_leaf()?;
        Some(leaf.tiled_edges)
    }

    pub fn tiled_scene_order(&self) -> Vec<ObjectId> {
        let mut order = Vec::new();
        if let Some(root_id) = self.root {
            self.collect_leaf_surfaces(root_id, &mut order);
        }
        order
    }

    pub fn floating_scene_order(&self) -> Vec<ObjectId> {
        self.floating.stack.clone()
    }

    pub fn focus_surface(&mut self, surface_object_id: ObjectId) -> bool {
        if let Some(&leaf_id) = self.surface_leaves.get(&surface_object_id) {
            self.focused_leaf = Some(leaf_id);
            self.update_focus_chain(leaf_id);
            return true;
        }
        if self.is_floating_surface(surface_object_id) {
            self.raise_floating_surface(surface_object_id);
            return true;
        }
        false
    }

    pub fn raise_floating_surface(&mut self, surface_object_id: ObjectId) -> bool {
        let Some(index) = self
            .floating
            .stack
            .iter()
            .position(|candidate| *candidate == surface_object_id)
        else {
            return false;
        };
        let surface_object_id = self.floating.stack.remove(index);
        self.floating.stack.push(surface_object_id);
        true
    }

    pub fn insert_tiled(&mut self, surface_object_id: ObjectId) -> bool {
        if self.contains_surface(surface_object_id) {
            return false;
        }

        let leaf_id = self.alloc_leaf(surface_object_id, None);
        let root_id = match self
            .focused_leaf
            .filter(|leaf_id| self.nodes.contains_key(leaf_id))
        {
            Some(focused_leaf_id) => self.insert_leaf_adjacent_to_focus(focused_leaf_id, leaf_id),
            None => match self.root {
                None => leaf_id,
                Some(root_id) => {
                    let axis = self
                        .nodes
                        .get(&root_id)
                        .and_then(LayoutNode::as_container)
                        .map(|container| container.axis)
                        .unwrap_or_default();
                    self.insert_leaf_under_root(root_id, leaf_id, axis)
                }
            },
        };

        self.root = Some(root_id);
        self.focus_surface(surface_object_id);
        self.mark_dirty();
        true
    }

    pub fn remove_surface(&mut self, surface_object_id: ObjectId) -> bool {
        let mut removed = false;
        if let Some(index) = self
            .floating
            .stack
            .iter()
            .position(|candidate| *candidate == surface_object_id)
        {
            self.floating.stack.remove(index);
            removed = true;
        }

        if self
            .fullscreen
            .as_ref()
            .is_some_and(|fullscreen| fullscreen.surface_object_id == surface_object_id)
        {
            self.fullscreen = None;
            removed = true;
        }

        if let Some(leaf_id) = self.surface_leaves.remove(&surface_object_id) {
            self.remove_leaf(leaf_id);
            removed = true;
        }

        if removed {
            self.mark_dirty();
        }
        removed
    }

    pub fn set_floating(&mut self, surface_object_id: ObjectId, floating: bool) -> bool {
        if floating {
            if self.is_floating_surface(surface_object_id) {
                return false;
            }
            self.remove_surface(surface_object_id);
            self.floating.stack.push(surface_object_id);
            self.raise_floating_surface(surface_object_id);
            self.mark_dirty();
            return true;
        }

        let Some(index) = self
            .floating
            .stack
            .iter()
            .position(|candidate| *candidate == surface_object_id)
        else {
            return false;
        };
        self.floating.stack.remove(index);
        self.insert_tiled(surface_object_id)
    }

    pub fn set_fullscreen(&mut self, fullscreen_surface: Option<ObjectId>) -> bool {
        let next =
            fullscreen_surface.map(|surface_object_id| FullscreenState { surface_object_id });
        let changed = self
            .fullscreen
            .as_ref()
            .map(|fullscreen| fullscreen.surface_object_id)
            != next.as_ref().map(|fullscreen| fullscreen.surface_object_id);
        if changed {
            self.fullscreen = next;
            self.mark_dirty();
        }
        changed
    }

    pub fn resize_tiled_surface(
        &mut self,
        surface_object_id: ObjectId,
        edges: u32,
        dx: i32,
        dy: i32,
    ) -> bool {
        let Some(resize_state) = self.begin_tiled_resize(surface_object_id, edges) else {
            return false;
        };
        self.resize_tiled_surface_from_state(surface_object_id, resize_state, dx, dy)
    }

    pub fn begin_tiled_resize(
        &self,
        surface_object_id: ObjectId,
        edges: u32,
    ) -> Option<TiledResizeState> {
        let Some(&leaf_id) = self.surface_leaves.get(&surface_object_id) else {
            return None;
        };
        let Some(parent_id) = self.nodes.get(&leaf_id).and_then(LayoutNode::parent) else {
            return None;
        };
        let Some((axis, children)) = self
            .nodes
            .get(&parent_id)
            .and_then(LayoutNode::as_container)
            .map(|container| (container.axis, container.children.clone()))
        else {
            return None;
        };
        let Some(index) = children.iter().position(|child| child.node_id == leaf_id) else {
            return None;
        };

        let neighbor_index = match axis {
            SplitAxis::Horizontal => {
                if edges & EDGE_LEFT != 0 && index > 0 {
                    index - 1
                } else if edges & EDGE_RIGHT != 0 && index + 1 < children.len() {
                    index + 1
                } else {
                    return None;
                }
            }
            SplitAxis::Vertical => {
                if edges & EDGE_TOP != 0 && index > 0 {
                    index - 1
                } else if edges & EDGE_BOTTOM != 0 && index + 1 < children.len() {
                    index + 1
                } else {
                    return None;
                }
            }
        };

        let axis_span = match axis {
            SplitAxis::Horizontal => {
                let Some(left) = self.leaf_rect(children[index].node_id) else {
                    return None;
                };
                let Some(right) = self.leaf_rect(children[neighbor_index].node_id) else {
                    return None;
                };
                left.width + right.width
            }
            SplitAxis::Vertical => {
                let Some(top) = self.leaf_rect(children[index].node_id) else {
                    return None;
                };
                let Some(bottom) = self.leaf_rect(children[neighbor_index].node_id) else {
                    return None;
                };
                top.height + bottom.height
            }
        };
        if axis_span <= 1 {
            return None;
        }

        let pair_weight = children[index].weight + children[neighbor_index].weight;
        Some(TiledResizeState {
            axis,
            leaf_index: index,
            neighbor_index,
            pair_weight,
            initial_leaf_weight: children[index].weight,
            axis_span,
        })
    }

    pub fn resize_tiled_surface_from_state(
        &mut self,
        surface_object_id: ObjectId,
        resize_state: TiledResizeState,
        dx: i32,
        dy: i32,
    ) -> bool {
        let Some(&leaf_id) = self.surface_leaves.get(&surface_object_id) else {
            return false;
        };
        let Some(parent_id) = self.nodes.get(&leaf_id).and_then(LayoutNode::parent) else {
            return false;
        };
        let Some(container) = self
            .nodes
            .get(&parent_id)
            .and_then(LayoutNode::as_container)
        else {
            return false;
        };
        if resize_state.leaf_index >= container.children.len()
            || resize_state.neighbor_index >= container.children.len()
            || container.children[resize_state.leaf_index].node_id != leaf_id
        {
            return false;
        }

        let delta_pixels = match resize_state.axis {
            SplitAxis::Horizontal => {
                if resize_state.neighbor_index < resize_state.leaf_index {
                    -dx
                } else {
                    dx
                }
            }
            SplitAxis::Vertical => {
                if resize_state.neighbor_index < resize_state.leaf_index {
                    -dy
                } else {
                    dy
                }
            }
        };

        let delta_weight =
            (delta_pixels as f64 / resize_state.axis_span as f64) * resize_state.pair_weight;
        let mut updated_leaf_weight = resize_state.initial_leaf_weight + delta_weight;
        updated_leaf_weight = updated_leaf_weight
            .clamp(MIN_CHILD_WEIGHT, resize_state.pair_weight - MIN_CHILD_WEIGHT);
        let updated_neighbor_weight = resize_state.pair_weight - updated_leaf_weight;

        let Some(LayoutNode::Container(container)) = self.nodes.get_mut(&parent_id) else {
            return false;
        };
        container.children[resize_state.leaf_index].weight = updated_leaf_weight;
        container.children[resize_state.neighbor_index].weight = updated_neighbor_weight;
        self.mark_dirty();
        true
    }

    pub fn resize_affected_surfaces(
        &self,
        surface_object_id: ObjectId,
        resize_state: TiledResizeState,
    ) -> Option<BTreeSet<ObjectId>> {
        let &leaf_id = self.surface_leaves.get(&surface_object_id)?;
        let parent_id = self.nodes.get(&leaf_id).and_then(LayoutNode::parent)?;
        let container = self
            .nodes
            .get(&parent_id)
            .and_then(LayoutNode::as_container)?;
        if resize_state.leaf_index >= container.children.len()
            || resize_state.neighbor_index >= container.children.len()
            || container.children[resize_state.leaf_index].node_id != leaf_id
        {
            return None;
        }

        let mut surfaces = Vec::new();
        self.collect_leaf_surfaces(
            container.children[resize_state.leaf_index].node_id,
            &mut surfaces,
        );
        self.collect_leaf_surfaces(
            container.children[resize_state.neighbor_index].node_id,
            &mut surfaces,
        );
        Some(surfaces.into_iter().collect())
    }

    fn alloc_leaf(
        &mut self,
        surface_object_id: ObjectId,
        parent: Option<LayoutNodeId>,
    ) -> LayoutNodeId {
        let id = self.alloc_node_id();
        self.nodes.insert(
            id,
            LayoutNode::Leaf(LeafNode {
                surface_object_id,
                parent,
                last_applied_rect: None,
                tiled_edges: 0,
            }),
        );
        self.surface_leaves.insert(surface_object_id, id);
        id
    }

    fn alloc_container(
        &mut self,
        axis: SplitAxis,
        parent: Option<LayoutNodeId>,
        children: Vec<LayoutChild>,
    ) -> LayoutNodeId {
        let id = self.alloc_node_id();
        self.nodes.insert(
            id,
            LayoutNode::Container(ContainerNode {
                axis,
                parent,
                focused_child: children.len().checked_sub(1),
                children,
            }),
        );
        id
    }

    fn alloc_node_id(&mut self) -> LayoutNodeId {
        let id = LayoutNodeId(self.next_node_id);
        self.next_node_id = self.next_node_id.saturating_add(1);
        id
    }

    fn insert_leaf_under_root(
        &mut self,
        root_id: LayoutNodeId,
        leaf_id: LayoutNodeId,
        axis: SplitAxis,
    ) -> LayoutNodeId {
        let root_is_container = matches!(self.nodes.get(&root_id), Some(LayoutNode::Container(_)));
        if root_is_container {
            let Some(LayoutNode::Container(container)) = self.nodes.get_mut(&root_id) else {
                unreachable!();
            };
            container
                .children
                .push(LayoutChild { node_id: leaf_id, weight: 1.0 });
            container.focused_child = Some(container.children.len() - 1);
            if let Some(node) = self.nodes.get_mut(&leaf_id) {
                node.set_parent(Some(root_id));
            }
            return root_id;
        }

        let new_root = self.alloc_container(
            axis,
            None,
            vec![
                LayoutChild { node_id: root_id, weight: 1.0 },
                LayoutChild { node_id: leaf_id, weight: 1.0 },
            ],
        );
        if let Some(node) = self.nodes.get_mut(&root_id) {
            node.set_parent(Some(new_root));
        }
        if let Some(node) = self.nodes.get_mut(&leaf_id) {
            node.set_parent(Some(new_root));
        }
        new_root
    }

    fn insert_leaf_adjacent_to_focus(
        &mut self,
        focused_leaf_id: LayoutNodeId,
        new_leaf_id: LayoutNodeId,
    ) -> LayoutNodeId {
        let desired_axis = self.preferred_insert_axis(focused_leaf_id);
        let parent_id = self
            .nodes
            .get(&focused_leaf_id)
            .and_then(LayoutNode::parent);
        if let Some(parent_id) = parent_id {
            let parent_axis = self
                .nodes
                .get(&parent_id)
                .and_then(LayoutNode::as_container)
                .map(|container| container.axis);
            if parent_axis == Some(desired_axis) {
                self.insert_leaf_into_container_after_focus(
                    parent_id,
                    focused_leaf_id,
                    new_leaf_id,
                );
                return self.root.unwrap_or(parent_id);
            }
            return self.wrap_focused_leaf_with_container(
                focused_leaf_id,
                new_leaf_id,
                Some(parent_id),
                desired_axis,
            );
        }
        self.wrap_focused_leaf_with_container(focused_leaf_id, new_leaf_id, None, desired_axis)
    }

    fn preferred_insert_axis(&self, focused_leaf_id: LayoutNodeId) -> SplitAxis {
        self.nodes
            .get(&focused_leaf_id)
            .and_then(LayoutNode::parent)
            .and_then(|parent_id| {
                self.nodes
                    .get(&parent_id)
                    .and_then(LayoutNode::as_container)
            })
            .map(|container| container.axis.opposite())
            .unwrap_or_default()
    }

    fn insert_leaf_into_container_after_focus(
        &mut self,
        container_id: LayoutNodeId,
        focused_leaf_id: LayoutNodeId,
        new_leaf_id: LayoutNodeId,
    ) {
        let Some(LayoutNode::Container(container)) = self.nodes.get_mut(&container_id) else {
            return;
        };
        let insert_index = container
            .children
            .iter()
            .position(|child| child.node_id == focused_leaf_id)
            .map(|index| index + 1)
            .unwrap_or(container.children.len());
        container
            .children
            .insert(insert_index, LayoutChild { node_id: new_leaf_id, weight: 1.0 });
        container.focused_child = Some(insert_index);
        if let Some(node) = self.nodes.get_mut(&new_leaf_id) {
            node.set_parent(Some(container_id));
        }
    }

    fn wrap_focused_leaf_with_container(
        &mut self,
        focused_leaf_id: LayoutNodeId,
        new_leaf_id: LayoutNodeId,
        parent_id: Option<LayoutNodeId>,
        axis: SplitAxis,
    ) -> LayoutNodeId {
        let new_container_id = self.alloc_container(
            axis,
            parent_id,
            vec![
                LayoutChild { node_id: focused_leaf_id, weight: 1.0 },
                LayoutChild { node_id: new_leaf_id, weight: 1.0 },
            ],
        );
        if let Some(node) = self.nodes.get_mut(&focused_leaf_id) {
            node.set_parent(Some(new_container_id));
        }
        if let Some(node) = self.nodes.get_mut(&new_leaf_id) {
            node.set_parent(Some(new_container_id));
        }
        if let Some(parent_id) = parent_id {
            if let Some(LayoutNode::Container(parent)) = self.nodes.get_mut(&parent_id) {
                if let Some(child_index) = parent
                    .children
                    .iter()
                    .position(|child| child.node_id == focused_leaf_id)
                {
                    parent.children[child_index].node_id = new_container_id;
                    parent.focused_child = Some(child_index);
                }
            }
            self.root
                .expect("root must exist when wrapping a leaf under an existing parent")
        } else {
            new_container_id
        }
    }

    fn remove_leaf(&mut self, leaf_id: LayoutNodeId) {
        let parent_id = self.nodes.get(&leaf_id).and_then(LayoutNode::parent);
        let removed_was_focused = self.focused_leaf == Some(leaf_id);
        self.nodes.remove(&leaf_id);
        if removed_was_focused {
            self.focused_leaf = None;
        }
        let Some(parent_id) = parent_id else {
            if self.root == Some(leaf_id) {
                self.root = None;
            }
            if removed_was_focused {
                self.focused_leaf = self
                    .root
                    .and_then(|root_id| self.first_leaf_in_subtree(root_id));
            }
            return;
        };

        let parent_snapshot = self
            .nodes
            .get(&parent_id)
            .and_then(LayoutNode::as_container)
            .cloned();
        let Some(parent_snapshot) = parent_snapshot else {
            return;
        };
        let remaining_children = parent_snapshot
            .children
            .iter()
            .filter(|child| child.node_id != leaf_id)
            .cloned()
            .collect::<Vec<_>>();
        let replacement_focus = removed_was_focused
            .then(|| self.replacement_focus_child(&parent_snapshot, &remaining_children, leaf_id))
            .flatten();
        if let Some(LayoutNode::Container(container)) = self.nodes.get_mut(&parent_id) {
            container.children = remaining_children;
            container.focused_child = (!container.children.is_empty()).then(|| {
                let focused_child = container.focused_child.unwrap_or(0);
                focused_child.min(container.children.len().saturating_sub(1))
            });
        }
        self.normalize_container(parent_id);
        if let Some(replacement_focus) = replacement_focus {
            self.focused_leaf = self.first_leaf_in_subtree(replacement_focus);
        } else if removed_was_focused {
            self.focused_leaf = self
                .root
                .and_then(|root_id| self.first_leaf_in_subtree(root_id));
        }
    }

    fn normalize_container(&mut self, container_id: LayoutNodeId) {
        let snapshot = self
            .nodes
            .get(&container_id)
            .and_then(LayoutNode::as_container)
            .cloned();
        let Some(snapshot) = snapshot else {
            return;
        };
        match snapshot.children.len() {
            0 => {
                let parent = snapshot.parent;
                self.nodes.remove(&container_id);
                if let Some(parent_id) = parent {
                    if let Some(LayoutNode::Container(container)) = self.nodes.get_mut(&parent_id) {
                        container
                            .children
                            .retain(|child| child.node_id != container_id);
                    }
                    self.normalize_container(parent_id);
                } else if self.root == Some(container_id) {
                    self.root = None;
                }
            }
            1 => {
                let promoted_id = snapshot.children[0].node_id;
                let parent = snapshot.parent;
                self.nodes.remove(&container_id);
                if let Some(node) = self.nodes.get_mut(&promoted_id) {
                    node.set_parent(parent);
                }
                if let Some(parent_id) = parent {
                    if let Some(LayoutNode::Container(container)) = self.nodes.get_mut(&parent_id) {
                        for child in &mut container.children {
                            if child.node_id == container_id {
                                child.node_id = promoted_id;
                            }
                        }
                    }
                    self.normalize_container(parent_id);
                } else {
                    self.root = Some(promoted_id);
                }
            }
            _ => {}
        }
    }

    fn update_focus_chain(&mut self, leaf_id: LayoutNodeId) {
        let mut current = Some(leaf_id);
        while let Some(node_id) = current {
            let parent_id = self.nodes.get(&node_id).and_then(LayoutNode::parent);
            let Some(parent_id) = parent_id else {
                break;
            };
            let child_index = self
                .nodes
                .get(&parent_id)
                .and_then(LayoutNode::as_container)
                .and_then(|container| {
                    container
                        .children
                        .iter()
                        .position(|child| child.node_id == node_id)
                });
            if let (Some(LayoutNode::Container(container)), Some(child_index)) =
                (self.nodes.get_mut(&parent_id), child_index)
            {
                container.focused_child = Some(child_index);
            }
            current = Some(parent_id);
        }
    }

    fn collect_leaf_surfaces(&self, node_id: LayoutNodeId, output: &mut Vec<ObjectId>) {
        let Some(node) = self.nodes.get(&node_id) else {
            return;
        };
        match node {
            LayoutNode::Leaf(leaf) => output.push(leaf.surface_object_id),
            LayoutNode::Container(container) => {
                for child in &container.children {
                    self.collect_leaf_surfaces(child.node_id, output);
                }
            }
        }
    }

    fn leaf_rect(&self, node_id: LayoutNodeId) -> Option<RectI> {
        self.nodes.get(&node_id)?.as_leaf()?.last_applied_rect
    }

    fn first_leaf_in_subtree(&self, node_id: LayoutNodeId) -> Option<LayoutNodeId> {
        match self.nodes.get(&node_id)? {
            LayoutNode::Leaf(_) => Some(node_id),
            LayoutNode::Container(container) => container
                .children
                .iter()
                .find_map(|child| self.first_leaf_in_subtree(child.node_id)),
        }
    }

    fn replacement_focus_child(
        &self,
        parent_snapshot: &ContainerNode,
        remaining_children: &[LayoutChild],
        removed_leaf_id: LayoutNodeId,
    ) -> Option<LayoutNodeId> {
        let removed_index = parent_snapshot
            .children
            .iter()
            .position(|child| child.node_id == removed_leaf_id)?;
        remaining_children
            .get(removed_index)
            .or_else(|| {
                removed_index
                    .checked_sub(1)
                    .and_then(|index| remaining_children.get(index))
            })
            .map(|child| child.node_id)
    }
}

impl LayoutNode {
    fn parent(&self) -> Option<LayoutNodeId> {
        match self {
            LayoutNode::Container(container) => container.parent,
            LayoutNode::Leaf(leaf) => leaf.parent,
        }
    }

    fn set_parent(&mut self, parent: Option<LayoutNodeId>) {
        match self {
            LayoutNode::Container(container) => container.parent = parent,
            LayoutNode::Leaf(leaf) => leaf.parent = parent,
        }
    }

    fn as_container(&self) -> Option<&ContainerNode> {
        match self {
            LayoutNode::Container(container) => Some(container),
            LayoutNode::Leaf(_) => None,
        }
    }

    fn as_leaf(&self) -> Option<&LeafNode> {
        match self {
            LayoutNode::Leaf(leaf) => Some(leaf),
            LayoutNode::Container(_) => None,
        }
    }
}

pub fn tiled_rect_for_surface(state: &State, surface_object_id: ObjectId) -> Option<RectI> {
    let workspace_id = state.root_surface_workspace(surface_object_id)?;
    let workspace = state.workspaces.try_get(workspace_id)?;
    workspace
        .layout
        .tiled_rect(surface_object_id)
        .or_else(|| {
            workspace
                .layout
                .fullscreen
                .as_ref()
                .is_some_and(|fullscreen| fullscreen.surface_object_id == surface_object_id)
                .then(|| workspace_usable_bounds(state, workspace_id))
                .flatten()
        })
        .map(|rect| {
            if state.root_surface_is_fullscreen(surface_object_id) {
                rect
            } else {
                content_rect_for_tiled_frame(
                    rect,
                    tiled_decoration_insets(state, surface_object_id),
                )
            }
        })
}

pub fn tiled_edges_for_surface(state: &State, surface_object_id: ObjectId) -> Option<u32> {
    let workspace_id = state.root_surface_workspace(surface_object_id)?;
    let workspace = state.workspaces.try_get(workspace_id)?;
    workspace.layout.tiled_edges(surface_object_id).or_else(|| {
        workspace
            .layout
            .fullscreen
            .as_ref()
            .is_some_and(|fullscreen| fullscreen.surface_object_id == surface_object_id)
            .then_some(EDGE_TOP | EDGE_BOTTOM | EDGE_LEFT | EDGE_RIGHT)
    })
}

pub fn initial_layout_configure_for_surface(
    state: &mut State,
    surface_object_id: ObjectId,
    surface_decoration_insets: DecorationInsets,
    target_min_size: Option<(i32, i32)>,
    configure_bounds: Option<(i32, i32)>,
) -> Option<InitialLayoutConfigure> {
    if !state.prepare_root_surface_initial_layout(surface_object_id) {
        return None;
    }
    let workspace_id = state.root_surface_workspace(surface_object_id)?;
    let workspace_surface_ids = state
        .managed_root_surfaces
        .iter()
        .filter_map(|(object_id, managed)| {
            (managed.workspace_id == workspace_id).then_some(*object_id)
        })
        .collect::<Vec<_>>();
    let candidate_surfaces = workspace_surface_ids
        .iter()
        .copied()
        .filter(|object_id| {
            if *object_id == surface_object_id {
                return state
                    .managed_root_surface_state(*object_id)
                    .is_some_and(|managed| managed.pending_initial_layout);
            }
            is_managed_root_surface_live(state, *object_id)
        })
        .collect::<Vec<_>>();
    let candidate_set = candidate_surfaces.iter().copied().collect::<BTreeSet<_>>();
    let candidate_modes = candidate_surfaces
        .iter()
        .filter_map(|object_id| {
            state
                .managed_root_surface_state(*object_id)
                .map(|managed| (*object_id, managed.mode))
        })
        .collect::<BTreeMap<_, _>>();
    let fullscreen_surface = workspace_surface_ids.iter().find_map(|object_id| {
        state
            .managed_root_surface_state(*object_id)
            .is_some_and(|managed| managed.fullscreen && candidate_set.contains(object_id))
            .then_some(*object_id)
    });
    let fullscreen_bounds = state
        .workspace_output_id(workspace_id)
        .and_then(|output_id| state.output_bounds(output_id))
        .map(|bounds| {
            let (width, height) = configure_bounds.unwrap_or((bounds.width, bounds.height));
            RectI { x: bounds.x, y: bounds.y, width, height }
        })
        .or_else(|| workspace_usable_bounds(state, workspace_id))?;
    let tiling_bounds = workspace_tiling_bounds(state, workspace_id, fullscreen_bounds);
    let min_sizes = state
        .managed_root_surfaces
        .iter()
        .filter_map(|(object_id, managed)| {
            (managed.workspace_id == workspace_id).then_some((
                *object_id,
                if *object_id == surface_object_id {
                    target_min_size.unwrap_or((1, 1))
                } else {
                    minimum_size_for_surface(state, *object_id)
                },
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let gap_policy = workspace_gap_policy(state, workspace_id);
    let mut decoration_insets = decoration_insets_for_surfaces(
        state,
        workspace_surface_ids
            .iter()
            .copied()
            .filter(|object_id| *object_id != surface_object_id),
    );
    decoration_insets.insert(surface_object_id, surface_decoration_insets);
    let mut predictive_layout = state.workspaces.try_get(workspace_id)?.layout.clone();
    sync_workspace_layout_candidates(
        &mut predictive_layout,
        &candidate_surfaces,
        &candidate_set,
        &candidate_modes,
        fullscreen_surface,
    );
    apply_workspace_layout(
        &mut predictive_layout,
        tiling_bounds,
        fullscreen_bounds,
        gap_policy,
        &decoration_insets,
        &min_sizes,
    );
    let frame_rect = predictive_layout
        .tiled_rect(surface_object_id)
        .or_else(|| {
            predictive_layout
                .fullscreen
                .as_ref()
                .is_some_and(|fullscreen| fullscreen.surface_object_id == surface_object_id)
                .then_some(fullscreen_bounds)
        })?;
    Some(InitialLayoutConfigure {
        frame_rect,
        tiled_edges: predictive_layout
            .tiled_edges(surface_object_id)
            .or_else(|| {
                predictive_layout
                    .fullscreen
                    .as_ref()
                    .is_some_and(|fullscreen| fullscreen.surface_object_id == surface_object_id)
                    .then_some(EDGE_TOP | EDGE_BOTTOM | EDGE_LEFT | EDGE_RIGHT)
            })
            .unwrap_or_default(),
        fullscreen: predictive_layout
            .fullscreen
            .as_ref()
            .is_some_and(|fullscreen| fullscreen.surface_object_id == surface_object_id),
    })
}

pub fn is_tiled_surface(state: &State, surface_object_id: ObjectId) -> bool {
    state
        .root_surface_workspace(surface_object_id)
        .and_then(|workspace_id| state.workspaces.try_get(workspace_id))
        .is_some_and(|workspace| workspace.layout.is_tiled_surface(surface_object_id))
}

pub fn is_floating_surface(state: &State, surface_object_id: ObjectId) -> bool {
    state
        .root_surface_workspace(surface_object_id)
        .and_then(|workspace_id| state.workspaces.try_get(workspace_id))
        .is_some_and(|workspace| workspace.layout.is_floating_surface(surface_object_id))
}

pub fn focus_surface(state: &mut State, surface_object_id: ObjectId) -> bool {
    let Some(workspace_id) = state.root_surface_workspace(surface_object_id) else {
        return false;
    };
    let Some(workspace) = state.workspaces.try_get_mut(workspace_id) else {
        return false;
    };
    workspace.layout.focus_surface(surface_object_id)
}

pub fn raise_surface(state: &mut State, surface_object_id: ObjectId) -> bool {
    let Some(workspace_id) = state.root_surface_workspace(surface_object_id) else {
        return false;
    };
    let Some(workspace) = state.workspaces.try_get_mut(workspace_id) else {
        return false;
    };
    if workspace.layout.is_tiled_surface(surface_object_id) {
        return workspace.layout.focus_surface(surface_object_id);
    }
    if workspace.layout.is_floating_surface(surface_object_id) {
        return workspace.layout.raise_floating_surface(surface_object_id);
    }
    false
}

pub fn set_surface_floating(
    state: &mut State,
    surface_object_id: ObjectId,
    floating: bool,
) -> bool {
    let Some(workspace_id) = state.root_surface_workspace(surface_object_id) else {
        return false;
    };
    let Some(workspace) = state.workspaces.try_get_mut(workspace_id) else {
        return false;
    };
    workspace.layout.set_floating(surface_object_id, floating)
}

pub fn resize_tiled_surface(
    state: &mut State,
    surface_object_id: ObjectId,
    edges: u32,
    dx: i32,
    dy: i32,
) -> bool {
    let Some(workspace_id) = state.root_surface_workspace(surface_object_id) else {
        return false;
    };
    let Some(workspace) = state.workspaces.try_get_mut(workspace_id) else {
        return false;
    };
    workspace
        .layout
        .resize_tiled_surface(surface_object_id, edges, dx, dy)
}

pub fn mark_workspace_dirty(state: &mut State, workspace_id: WorkspaceId) {
    if let Some(workspace) = state.workspaces.try_get_mut(workspace_id) {
        workspace.layout.mark_dirty();
    }
}

pub fn managed_scene_order(state: &State) -> Vec<ObjectId> {
    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();
    for output_id in state.outputs.iter().map(|(output_id, _)| output_id) {
        let Some(workspace_id) = state.active_workspace_for_output(output_id) else {
            continue;
        };
        let Some(workspace) = state.workspaces.try_get(workspace_id) else {
            continue;
        };
        if let Some(fullscreen) = workspace
            .layout
            .fullscreen
            .as_ref()
            .filter(|fullscreen| state.root_surface_is_mapped(fullscreen.surface_object_id))
        {
            if seen.insert(fullscreen.surface_object_id) {
                ordered.push(fullscreen.surface_object_id);
            }
            continue;
        }
        for surface_object_id in workspace.layout.tiled_scene_order() {
            if seen.insert(surface_object_id) {
                ordered.push(surface_object_id);
            }
        }
        for surface_object_id in workspace.layout.floating_scene_order() {
            if seen.insert(surface_object_id) {
                ordered.push(surface_object_id);
            }
        }
    }
    ordered
}

pub fn relayout_dirty_workspaces(state: &mut State) {
    let workspace_ids = state
        .workspace_ids()
        .into_iter()
        .filter(|workspace_id| {
            state.workspace_is_active(*workspace_id)
                || state
                    .workspaces
                    .try_get(*workspace_id)
                    .is_some_and(|workspace| workspace.layout.dirty)
        })
        .collect::<Vec<_>>();
    for workspace_id in workspace_ids.iter().copied() {
        sync_workspace_layout(state, workspace_id);
    }
    for workspace_id in workspace_ids {
        let Some(fullscreen_bounds) = workspace_usable_bounds(state, workspace_id) else {
            continue;
        };
        let tiling_bounds = workspace_tiling_bounds(state, workspace_id, fullscreen_bounds);
        let min_sizes = workspace_minimum_sizes(state, workspace_id);
        let gap_policy = workspace_gap_policy(state, workspace_id);
        let decoration_insets = decoration_insets_for_surfaces(
            state,
            state
                .managed_root_surfaces
                .iter()
                .filter_map(|(object_id, managed)| {
                    (managed.workspace_id == workspace_id).then_some(*object_id)
                }),
        );
        let operations = {
            let Some(workspace) = state.workspaces.try_get_mut(workspace_id) else {
                continue;
            };
            apply_workspace_layout(
                &mut workspace.layout,
                tiling_bounds,
                fullscreen_bounds,
                gap_policy,
                &decoration_insets,
                &min_sizes,
            )
        };
        for operation in operations {
            let Some(mut surface) = state
                .objects
                .try_grab_object::<WlSurface>(operation.surface_object_id)
            else {
                continue;
            };
            let (rx, ry) = tiled_root_position_for_layout(
                state,
                operation.surface_object_id,
                &surface,
                operation.frame_rect,
                operation.content_rect,
            );
            surface.set_root_position(rx, ry);
            debug_log_tiled_geometry_snapshot(
                state,
                operation.surface_object_id,
                &surface,
                operation.frame_rect,
                operation.content_rect,
                decoration_insets_for_surface(&decoration_insets, operation.surface_object_id),
            );
        }
    }
}

fn workspace_usable_bounds(state: &State, workspace_id: WorkspaceId) -> Option<RectI> {
    let output_id = state.workspace_output_id(workspace_id)?;
    output_usable_bounds(state, output_id).or_else(|| state.output_bounds(output_id))
}

fn workspace_gap_policy(state: &State, _workspace_id: WorkspaceId) -> GapPolicy {
    GapPolicy {
        outer_gap: state.config.general.outer_gap,
        inner_gap: state.config.general.inner_gap,
    }
}

fn tiled_decoration_insets(state: &State, surface_object_id: ObjectId) -> DecorationInsets {
    effective_decoration_insets(state, surface_object_id)
}

fn decoration_insets_for_surface(
    decoration_insets: &DecorationInsetsMap,
    surface_object_id: ObjectId,
) -> DecorationInsets {
    decoration_insets
        .get(&surface_object_id)
        .copied()
        .unwrap_or_default()
}

fn decoration_insets_for_surfaces<I>(state: &State, surface_object_ids: I) -> DecorationInsetsMap
where
    I: IntoIterator<Item = ObjectId>,
{
    surface_object_ids
        .into_iter()
        .map(|surface_object_id| {
            (surface_object_id, effective_decoration_insets(state, surface_object_id))
        })
        .collect()
}

pub(crate) fn effective_decoration_insets(
    state: &State,
    surface_object_id: ObjectId,
) -> DecorationInsets {
    let Some(surface) = state.objects.try_ref_object::<WlSurface>(surface_object_id) else {
        return state.config.general.decoration_insets;
    };
    effective_decoration_insets_for_surface(state, surface_object_id, surface)
}

pub(crate) fn effective_decoration_insets_for_surface(
    state: &State,
    surface_object_id: ObjectId,
    surface: &WlSurface,
) -> DecorationInsets {
    let fallback = state.config.general.decoration_insets;
    let Some((_, toplevel)) =
        xdg_toplevel_for_surface_ids(state, surface.client_id(), surface.xdg_surface_id())
    else {
        return fallback;
    };
    if toplevel.decoration_mode() == ToplevelDecorationMode::ServerSide {
        return fallback;
    }
    let Some(explicit_geometry) = toplevel.window_geometry() else {
        return fallback;
    };
    let Some(window_geometry) = effective_window_geometry_for_surface(
        state,
        surface_object_id,
        surface,
        Some(explicit_geometry),
    ) else {
        return fallback;
    };
    decoration_insets_from_window_geometry(surface, window_geometry).unwrap_or(fallback)
}

pub(crate) fn effective_window_geometry_for_surface(
    state: &State,
    surface_object_id: ObjectId,
    surface: &WlSurface,
    window_geometry: Option<RectI>,
) -> Option<RectI> {
    let surface_bounds =
        WlSurface::local_surface_tree_bounds_with_surface(state, surface_object_id, surface)?;
    window_geometry
        .map(|raw| clamp_window_geometry_to_surface_bounds(raw, surface_bounds))
        .or(Some(surface_bounds))
}

fn debug_log_tiled_geometry_snapshot(
    state: &State,
    surface_object_id: ObjectId,
    surface: &WlSurface,
    frame_rect: RectI,
    content_rect: RectI,
    insets: DecorationInsets,
) {
    let (root_x, root_y) = surface.root_position();
    let surface_bounds =
        WlSurface::local_surface_tree_bounds_with_surface(state, surface_object_id, surface);
    let logical_bounds = surface.global_logical_bounds();
    let logical_size = surface.logical_size();
    let (decoration_mode, explicit_window_geometry, effective_window_geometry) =
        xdg_toplevel_for_surface_ids(state, surface.client_id(), surface.xdg_surface_id())
            .map(|(_, toplevel)| {
                let explicit = toplevel.window_geometry();
                (
                    Some(toplevel.decoration_mode()),
                    explicit,
                    effective_window_geometry_for_surface(
                        state,
                        surface_object_id,
                        surface,
                        explicit,
                    ),
                )
            })
            .unwrap_or((None, None, None));

    let geometry_anchor_ok = effective_window_geometry.is_some_and(|geom| {
        root_x.saturating_add(geom.x) == frame_rect.x
            && root_y.saturating_add(geom.y) == frame_rect.y
    });
    let shadow_outsets = surface_bounds
        .zip(effective_window_geometry)
        .map(|(bounds, geom)| DecorationInsets {
            left: geom.x.saturating_sub(bounds.x).max(0),
            top: geom.y.saturating_sub(bounds.y).max(0),
            right: bounds.x2().saturating_sub(geom.x2()).max(0),
            bottom: bounds.y2().saturating_sub(geom.y2()).max(0),
        });

    log::debug!(
        "[DEBUG_TILING] surface={:?} root=({}, {}) frame={:?} content={:?} insets={:?} logical_size={:?} logical_bounds={:?} surface_bounds={:?} decoration_mode={:?} explicit_window_geometry={:?} effective_window_geometry={:?} shadow_outsets={:?} geometry_anchor_ok={}",
        surface_object_id,
        root_x,
        root_y,
        frame_rect,
        content_rect,
        insets,
        logical_size,
        logical_bounds,
        surface_bounds,
        decoration_mode,
        explicit_window_geometry,
        effective_window_geometry,
        shadow_outsets,
        geometry_anchor_ok,
    );
}

/// Places the wl_surface root so explicit xdg window geometry aligns with the tile frame.
/// When the client never set window geometry, fall back to placing the root buffer at the
/// content rect derived from configured decoration insets.
pub(crate) fn tiled_root_position_for_layout(
    state: &State,
    surface_object_id: ObjectId,
    surface: &WlSurface,
    frame_rect: RectI,
    content_rect: RectI,
) -> (i32, i32) {
    let Some((_, toplevel)) =
        xdg_toplevel_for_surface_ids(state, surface.client_id(), surface.xdg_surface_id())
    else {
        return (content_rect.x, content_rect.y);
    };
    let Some(explicit_geometry) = toplevel.window_geometry() else {
        return (content_rect.x, content_rect.y);
    };
    let Some(window_geometry) = effective_window_geometry_for_surface(
        state,
        surface_object_id,
        surface,
        Some(explicit_geometry),
    ) else {
        return (content_rect.x, content_rect.y);
    };
    (frame_rect.x.saturating_sub(window_geometry.x), frame_rect.y.saturating_sub(window_geometry.y))
}

fn clamp_window_geometry_to_surface_bounds(window_geometry: RectI, surface_bounds: RectI) -> RectI {
    let x1 = window_geometry
        .x
        .clamp(surface_bounds.x, surface_bounds.x2());
    let y1 = window_geometry
        .y
        .clamp(surface_bounds.y, surface_bounds.y2());
    let x2 = window_geometry
        .x2()
        .clamp(surface_bounds.x, surface_bounds.x2());
    let y2 = window_geometry
        .y2()
        .clamp(surface_bounds.y, surface_bounds.y2());
    RectI::from_extents(x1, y1, x2, y2).unwrap_or(surface_bounds)
}

pub(crate) fn decoration_insets_from_window_geometry(
    surface: &WlSurface,
    window_geometry: RectI,
) -> Option<DecorationInsets> {
    let logical_size = surface.logical_size()?;
    decoration_insets_from_window_geometry_and_logical_size(logical_size, window_geometry)
}

pub(crate) fn decoration_insets_from_window_geometry_and_logical_size(
    logical_size: SizeI,
    window_geometry: RectI,
) -> Option<DecorationInsets> {
    let w = logical_size.width;
    let h = logical_size.height;
    let gx2 = window_geometry.x2();
    let gy2 = window_geometry.y2();
    Some(DecorationInsets {
        // Padding between the surface buffer [0,w)×[0,h) and the window-geometry rectangle:
        // negative origin (GTK-style subsurface CSD) and positive origin (content inset from
        // buffer top-left, e.g. title bar above geometry) are both covered.
        left: window_geometry
            .x
            .saturating_neg()
            .max(0)
            .saturating_add(window_geometry.x.max(0)),
        top: window_geometry
            .y
            .saturating_neg()
            .max(0)
            .saturating_add(window_geometry.y.max(0)),
        right: gx2
            .saturating_sub(w)
            .max(0)
            .saturating_add(w.saturating_sub(gx2).max(0)),
        bottom: gy2
            .saturating_sub(h)
            .max(0)
            .saturating_add(h.saturating_sub(gy2).max(0)),
    })
}

fn workspace_tiling_bounds(
    state: &State,
    workspace_id: WorkspaceId,
    usable_bounds: RectI,
) -> RectI {
    let gap_policy = workspace_gap_policy(state, workspace_id);
    outer_gapped_bounds(usable_bounds, gap_policy)
}

fn sync_workspace_layout(state: &mut State, workspace_id: WorkspaceId) {
    let workspace_surface_ids = state
        .managed_root_surfaces
        .iter()
        .filter_map(|(object_id, managed)| {
            (managed.workspace_id == workspace_id).then_some(*object_id)
        })
        .collect::<Vec<_>>();
    let candidate_surfaces = workspace_surface_ids
        .iter()
        .copied()
        .filter(|object_id| is_managed_root_surface_live(state, *object_id))
        .collect::<Vec<_>>();
    let candidate_set = candidate_surfaces.iter().copied().collect::<BTreeSet<_>>();
    let candidate_modes = candidate_surfaces
        .iter()
        .filter_map(|object_id| {
            state
                .managed_root_surfaces
                .get(object_id)
                .map(|managed| (*object_id, managed.mode))
        })
        .collect::<BTreeMap<_, _>>();
    for object_id in &workspace_surface_ids {
        if let Some(managed) = state.managed_root_surfaces.get_mut(object_id) {
            managed.mapped = candidate_set.contains(object_id);
        }
    }

    let fullscreen_surface = workspace_surface_ids.iter().find_map(|object_id| {
        state
            .managed_root_surfaces
            .get(object_id)
            .is_some_and(|managed| managed.fullscreen && managed.mapped)
            .then_some(*object_id)
    });

    let Some(workspace) = state.workspaces.try_get_mut(workspace_id) else {
        return;
    };

    sync_workspace_layout_candidates(
        &mut workspace.layout,
        &candidate_surfaces,
        &candidate_set,
        &candidate_modes,
        fullscreen_surface,
    );
}

fn sync_workspace_layout_candidates(
    layout: &mut WorkspaceLayout,
    candidate_surfaces: &[ObjectId],
    candidate_set: &BTreeSet<ObjectId>,
    candidate_modes: &BTreeMap<ObjectId, ManagedWindowMode>,
    fullscreen_surface: Option<ObjectId>,
) {
    let current_surfaces = layout
        .surface_leaves
        .keys()
        .copied()
        .chain(layout.floating.stack.iter().copied())
        .collect::<BTreeSet<_>>();
    for removed_surface in current_surfaces
        .difference(&candidate_set)
        .copied()
        .collect::<Vec<_>>()
    {
        layout.remove_surface(removed_surface);
    }
    for surface_object_id in candidate_surfaces.iter().copied() {
        let mode = candidate_modes
            .get(&surface_object_id)
            .copied()
            .unwrap_or(ManagedWindowMode::Tiled);
        match mode {
            ManagedWindowMode::Tiled => {
                if layout.is_floating_surface(surface_object_id) {
                    layout.set_floating(surface_object_id, false);
                } else if !layout.contains_surface(surface_object_id) {
                    layout.insert_tiled(surface_object_id);
                }
            }
            ManagedWindowMode::Floating => {
                if !layout.is_floating_surface(surface_object_id) {
                    layout.set_floating(surface_object_id, true);
                }
            }
        }
    }
    layout.set_fullscreen(fullscreen_surface);
}

fn apply_workspace_layout(
    layout: &mut WorkspaceLayout,
    tiling_bounds: RectI,
    fullscreen_bounds: RectI,
    gap_policy: GapPolicy,
    decoration_insets: &DecorationInsetsMap,
    min_sizes: &BTreeMap<ObjectId, (i32, i32)>,
) -> Vec<LayoutOperation> {
    let mut operations = Vec::new();
    if let Some(fullscreen) = layout.fullscreen.as_ref() {
        if let Some(&leaf_id) = layout.surface_leaves.get(&fullscreen.surface_object_id) {
            if let Some(LayoutNode::Leaf(leaf)) = layout.nodes.get_mut(&leaf_id) {
                leaf.last_applied_rect = Some(fullscreen_bounds);
                leaf.tiled_edges = EDGE_TOP | EDGE_BOTTOM | EDGE_LEFT | EDGE_RIGHT;
            }
        }
        operations.push(LayoutOperation {
            surface_object_id: fullscreen.surface_object_id,
            frame_rect: fullscreen_bounds,
            content_rect: fullscreen_bounds,
        });
        layout.dirty = false;
        return operations;
    }

    if let Some(root_id) = layout.root {
        let edge_constraints = workspace_edge_constraints(tiling_bounds);
        apply_node_layout(
            layout,
            root_id,
            tiling_bounds,
            tiling_bounds,
            edge_constraints,
            gap_policy,
            decoration_insets,
            min_sizes,
            &mut operations,
        );
    }
    layout.dirty = false;
    operations
}

fn apply_node_layout(
    layout: &mut WorkspaceLayout,
    node_id: LayoutNodeId,
    rect: RectI,
    workspace_bounds: RectI,
    edge_constraints: TiledEdgeConstraints,
    gap_policy: GapPolicy,
    decoration_insets: &DecorationInsetsMap,
    min_sizes: &BTreeMap<ObjectId, (i32, i32)>,
    operations: &mut Vec<LayoutOperation>,
) {
    let Some(node_snapshot) = layout.nodes.get(&node_id).cloned() else {
        return;
    };
    match node_snapshot {
        LayoutNode::Leaf(leaf) => {
            if let Some(LayoutNode::Leaf(leaf_mut)) = layout.nodes.get_mut(&node_id) {
                leaf_mut.last_applied_rect = Some(rect);
                leaf_mut.tiled_edges =
                    tiled_edges_for_rect(rect, workspace_bounds, edge_constraints);
            }
            operations.push(LayoutOperation {
                surface_object_id: leaf.surface_object_id,
                frame_rect: rect,
                content_rect: content_rect_for_tiled_frame(
                    rect,
                    decoration_insets_for_surface(decoration_insets, leaf.surface_object_id),
                ),
            });
        }
        LayoutNode::Container(container) => {
            let axis_span = match container.axis {
                SplitAxis::Horizontal => rect.width,
                SplitAxis::Vertical => rect.height,
            };
            let inner_gap =
                effective_inner_gap(gap_policy.inner_gap, container.children.len(), axis_span);
            let main_sizes = compute_child_main_sizes(
                layout,
                &container,
                rect,
                gap_policy,
                decoration_insets,
                min_sizes,
            );
            let mut cursor_x = rect.x;
            let mut cursor_y = rect.y;
            for (index, child) in container.children.iter().enumerate() {
                let main_size = main_sizes.get(index).copied().unwrap_or(0);
                let child_rect = match container.axis {
                    SplitAxis::Horizontal => {
                        let child_rect = RectI {
                            x: cursor_x,
                            y: rect.y,
                            width: main_size,
                            height: rect.height,
                        };
                        cursor_x += main_size;
                        if index + 1 != container.children.len() {
                            cursor_x += inner_gap;
                        }
                        child_rect
                    }
                    SplitAxis::Vertical => {
                        let child_rect =
                            RectI { x: rect.x, y: cursor_y, width: rect.width, height: main_size };
                        cursor_y += main_size;
                        if index + 1 != container.children.len() {
                            cursor_y += inner_gap;
                        }
                        child_rect
                    }
                };
                let child_edge_constraints =
                    child_edge_constraints(edge_constraints, &container, child_rect, index);
                apply_node_layout(
                    layout,
                    child.node_id,
                    child_rect,
                    workspace_bounds,
                    child_edge_constraints,
                    gap_policy,
                    decoration_insets,
                    min_sizes,
                    operations,
                );
            }
        }
    }
}

fn tiled_edges_for_rect(
    rect: RectI,
    workspace_bounds: RectI,
    edge_constraints: TiledEdgeConstraints,
) -> u32 {
    let mut edges = 0;
    if edge_constraints
        .left
        .is_some_and(|left| rect.x <= left && left >= workspace_bounds.x)
    {
        edges |= EDGE_LEFT;
    }
    if edge_constraints
        .top
        .is_some_and(|top| rect.y <= top && top >= workspace_bounds.y)
    {
        edges |= EDGE_TOP;
    }
    if edge_constraints
        .right
        .is_some_and(|right| rect.x2() >= right && right <= workspace_bounds.x2())
    {
        edges |= EDGE_RIGHT;
    }
    if edge_constraints
        .bottom
        .is_some_and(|bottom| rect.y2() >= bottom && bottom <= workspace_bounds.y2())
    {
        edges |= EDGE_BOTTOM;
    }
    edges
}

fn workspace_edge_constraints(workspace_bounds: RectI) -> TiledEdgeConstraints {
    TiledEdgeConstraints {
        left: Some(workspace_bounds.x),
        right: Some(workspace_bounds.x2()),
        top: Some(workspace_bounds.y),
        bottom: Some(workspace_bounds.y2()),
    }
}

fn child_edge_constraints(
    parent_constraints: TiledEdgeConstraints,
    container: &ContainerNode,
    child_rect: RectI,
    index: usize,
) -> TiledEdgeConstraints {
    let mut constraints = parent_constraints;
    match container.axis {
        SplitAxis::Horizontal => {
            if index > 0 {
                constraints.left = Some(child_rect.x);
            }
            if index + 1 != container.children.len() {
                constraints.right = Some(child_rect.x2());
            }
        }
        SplitAxis::Vertical => {
            if index > 0 {
                constraints.top = Some(child_rect.y);
            }
            if index + 1 != container.children.len() {
                constraints.bottom = Some(child_rect.y2());
            }
        }
    }
    constraints
}

fn compute_child_main_sizes(
    layout: &WorkspaceLayout,
    container: &ContainerNode,
    rect: RectI,
    gap_policy: GapPolicy,
    decoration_insets: &DecorationInsetsMap,
    min_sizes: &BTreeMap<ObjectId, (i32, i32)>,
) -> Vec<i32> {
    let span = match container.axis {
        SplitAxis::Horizontal => rect.width,
        SplitAxis::Vertical => rect.height,
    }
    .max(0);
    if container.children.is_empty() {
        return Vec::new();
    }
    let inner_gap = effective_inner_gap(gap_policy.inner_gap, container.children.len(), span);
    let total_gap = total_inner_gap(inner_gap, container.children.len());
    let available = span.saturating_sub(total_gap).max(0);

    let preferred_sizes = weighted_child_main_sizes(container, available);
    let minimums = container
        .children
        .iter()
        .enumerate()
        .map(|(index, child)| {
            child_minimum_main_size(
                layout,
                child.node_id,
                container.axis,
                child_size_hint(
                    container.axis,
                    rect,
                    preferred_sizes.get(index).copied().unwrap_or(0),
                ),
                gap_policy,
                decoration_insets,
                min_sizes,
            )
        })
        .collect::<Vec<_>>();
    let effective_minimums = if minimums.iter().sum::<i32>() <= available {
        minimums
    } else {
        // If client minimums do not fit, fall back to weighted distribution with a 1px floor.
        vec![1; container.children.len()]
    };

    let mut sizes = Vec::with_capacity(container.children.len());
    let mut consumed = 0;
    for index in 0..container.children.len() {
        let remaining_available = available.saturating_sub(consumed);
        if index + 1 == container.children.len() {
            sizes.push(remaining_available);
            break;
        }
        let remaining_required = effective_minimums[index + 1..].iter().sum::<i32>();
        let max_size = remaining_available
            .saturating_sub(remaining_required)
            .max(0);
        let min_size = effective_minimums[index].min(max_size);
        let size = preferred_sizes[index].clamp(min_size, max_size);
        sizes.push(size);
        consumed += size;
    }
    sizes
}

fn child_minimum_main_size(
    layout: &WorkspaceLayout,
    node_id: LayoutNodeId,
    axis: SplitAxis,
    size_hint: LayoutSizeHint,
    gap_policy: GapPolicy,
    decoration_insets: &DecorationInsetsMap,
    min_sizes: &BTreeMap<ObjectId, (i32, i32)>,
) -> i32 {
    let (min_width, min_height) = node_minimum_size(
        layout,
        node_id,
        Some(size_hint),
        gap_policy,
        decoration_insets,
        min_sizes,
    );
    match axis {
        SplitAxis::Horizontal => min_width,
        SplitAxis::Vertical => min_height,
    }
}

fn node_minimum_size(
    layout: &WorkspaceLayout,
    node_id: LayoutNodeId,
    size_hint: Option<LayoutSizeHint>,
    gap_policy: GapPolicy,
    decoration_insets: &DecorationInsetsMap,
    min_sizes: &BTreeMap<ObjectId, (i32, i32)>,
) -> (i32, i32) {
    let Some(node) = layout.nodes.get(&node_id) else {
        return (1, 1);
    };
    match node {
        LayoutNode::Leaf(leaf) => {
            let (min_width, min_height) = min_sizes
                .get(&leaf.surface_object_id)
                .copied()
                .unwrap_or((1, 1));
            let insets = decoration_insets_for_surface(decoration_insets, leaf.surface_object_id);
            (
                min_width
                    .saturating_add(insets.left)
                    .saturating_add(insets.right)
                    .max(1),
                min_height
                    .saturating_add(insets.top)
                    .saturating_add(insets.bottom)
                    .max(1),
            )
        }
        LayoutNode::Container(container) => {
            let effective_gap = size_hint
                .map(|hint| {
                    effective_inner_gap_for_container(container.axis, hint, gap_policy, container)
                })
                .unwrap_or_else(|| gap_policy.inner_gap.max(0));
            let child_hints = size_hint
                .map(|hint| child_size_hint_for_container(container, hint, gap_policy))
                .unwrap_or_else(|| vec![LayoutSizeHint::default(); container.children.len()]);
            let child_sizes = container
                .children
                .iter()
                .zip(child_hints)
                .map(|(child, child_hint)| {
                    node_minimum_size(
                        layout,
                        child.node_id,
                        size_hint.map(|_| child_hint),
                        gap_policy,
                        decoration_insets,
                        min_sizes,
                    )
                })
                .collect::<Vec<_>>();
            let total_gap = total_inner_gap(effective_gap, container.children.len());
            match container.axis {
                SplitAxis::Horizontal => (
                    child_sizes
                        .iter()
                        .map(|(width, _)| *width)
                        .sum::<i32>()
                        .saturating_add(total_gap)
                        .max(1),
                    child_sizes
                        .iter()
                        .map(|(_, height)| *height)
                        .max()
                        .unwrap_or(1),
                ),
                SplitAxis::Vertical => (
                    child_sizes
                        .iter()
                        .map(|(width, _)| *width)
                        .max()
                        .unwrap_or(1),
                    child_sizes
                        .iter()
                        .map(|(_, height)| *height)
                        .sum::<i32>()
                        .saturating_add(total_gap)
                        .max(1),
                ),
            }
        }
    }
}

fn effective_inner_gap_for_container(
    axis: SplitAxis,
    size_hint: LayoutSizeHint,
    gap_policy: GapPolicy,
    container: &ContainerNode,
) -> i32 {
    let axis_span = match axis {
        SplitAxis::Horizontal => size_hint.width,
        SplitAxis::Vertical => size_hint.height,
    };
    effective_inner_gap(gap_policy.inner_gap, container.children.len(), axis_span)
}

fn child_size_hint(axis: SplitAxis, rect: RectI, main_size: i32) -> LayoutSizeHint {
    match axis {
        SplitAxis::Horizontal => {
            LayoutSizeHint { width: main_size.max(0), height: rect.height.max(0) }
        }
        SplitAxis::Vertical => {
            LayoutSizeHint { width: rect.width.max(0), height: main_size.max(0) }
        }
    }
}

fn child_size_hint_for_container(
    container: &ContainerNode,
    size_hint: LayoutSizeHint,
    gap_policy: GapPolicy,
) -> Vec<LayoutSizeHint> {
    let axis_span = match container.axis {
        SplitAxis::Horizontal => size_hint.width,
        SplitAxis::Vertical => size_hint.height,
    }
    .max(0);
    let inner_gap =
        effective_inner_gap_for_container(container.axis, size_hint, gap_policy, container);
    let available = axis_span.saturating_sub(total_inner_gap(inner_gap, container.children.len()));
    weighted_child_main_sizes(container, available)
        .into_iter()
        .map(|main_size| match container.axis {
            SplitAxis::Horizontal => {
                LayoutSizeHint { width: main_size, height: size_hint.height.max(0) }
            }
            SplitAxis::Vertical => {
                LayoutSizeHint { width: size_hint.width.max(0), height: main_size }
            }
        })
        .collect()
}

fn weighted_child_main_sizes(container: &ContainerNode, available: i32) -> Vec<i32> {
    let total_weight = container
        .children
        .iter()
        .map(|child| child.weight.max(MIN_CHILD_WEIGHT))
        .sum::<f64>()
        .max(MIN_CHILD_WEIGHT);
    container
        .children
        .iter()
        .map(|child| {
            (((available as f64) * child.weight.max(MIN_CHILD_WEIGHT)) / total_weight).round()
                as i32
        })
        .collect::<Vec<_>>()
}

fn content_rect_for_tiled_frame(frame_rect: RectI, insets: DecorationInsets) -> RectI {
    shrink_rect(frame_rect, insets)
}

fn outer_gapped_bounds(usable_bounds: RectI, gap_policy: GapPolicy) -> RectI {
    shrink_rect(
        usable_bounds,
        DecorationInsets {
            top: gap_policy.outer_gap,
            right: gap_policy.outer_gap,
            bottom: gap_policy.outer_gap,
            left: gap_policy.outer_gap,
        },
    )
}

fn shrink_rect(rect: RectI, insets: DecorationInsets) -> RectI {
    if rect.width <= 0 || rect.height <= 0 {
        return rect;
    }

    let max_left = rect.width.saturating_sub(1);
    let left = insets.left.clamp(0, max_left);
    let remaining_width = rect.width.saturating_sub(left);
    let right = insets.right.clamp(0, remaining_width.saturating_sub(1));

    let max_top = rect.height.saturating_sub(1);
    let top = insets.top.clamp(0, max_top);
    let remaining_height = rect.height.saturating_sub(top);
    let bottom = insets.bottom.clamp(0, remaining_height.saturating_sub(1));

    RectI {
        x: rect.x.saturating_add(left),
        y: rect.y.saturating_add(top),
        width: rect.width.saturating_sub(left).saturating_sub(right).max(1),
        height: rect
            .height
            .saturating_sub(top)
            .saturating_sub(bottom)
            .max(1),
    }
}

fn effective_inner_gap(requested_gap: i32, child_count: usize, axis_span: i32) -> i32 {
    let requested_gap = requested_gap.max(0);
    let gap_count = child_count.saturating_sub(1);
    if gap_count == 0 {
        return 0;
    }
    let span = axis_span.max(0);
    let gap_count = gap_count as i32;
    requested_gap.min(span.saturating_div(gap_count))
}

fn total_inner_gap(inner_gap: i32, child_count: usize) -> i32 {
    inner_gap
        .max(0)
        .saturating_mul(child_count.saturating_sub(1) as i32)
}

fn workspace_minimum_sizes(
    state: &State,
    workspace_id: WorkspaceId,
) -> BTreeMap<ObjectId, (i32, i32)> {
    state
        .managed_root_surfaces
        .iter()
        .filter_map(|(object_id, managed)| {
            (managed.workspace_id == workspace_id)
                .then_some((*object_id, minimum_size_for_surface(state, *object_id)))
        })
        .collect()
}

fn minimum_size_for_surface(state: &State, surface_object_id: ObjectId) -> (i32, i32) {
    let Some(surface) = state.objects.try_ref_object::<WlSurface>(surface_object_id) else {
        return (1, 1);
    };
    let Some((_, toplevel)) =
        xdg_toplevel_for_xdg_surface(state, surface_object_id, surface.xdg_surface_id())
    else {
        return (1, 1);
    };
    toplevel.minimum_size().unwrap_or((1, 1))
}

fn xdg_toplevel_for_surface_ids(
    state: &State,
    client_id: crate::clients::ClientId,
    xdg_surface_id: Option<XdgSurfaceId>,
) -> Option<(ObjectId, &XdgToplevel)> {
    let xdg_surface_id = xdg_surface_id?;
    let object_id = state.xdg_toplevel_object_id(client_id, xdg_surface_id)?;
    let toplevel = state.objects.try_ref_object::<XdgToplevel>(object_id)?;
    Some((object_id, toplevel))
}

fn xdg_toplevel_for_xdg_surface(
    state: &State,
    surface_object_id: ObjectId,
    xdg_surface_id: Option<XdgSurfaceId>,
) -> Option<(ObjectId, &XdgToplevel)> {
    let xdg_surface_id = xdg_surface_id?;
    let surface = state
        .objects
        .try_ref_object::<WlSurface>(surface_object_id)?;
    xdg_toplevel_for_surface_ids(state, surface.client_id(), Some(xdg_surface_id))
}

fn is_managed_root_surface_live(state: &State, surface_object_id: ObjectId) -> bool {
    let Some(surface) = state.objects.try_ref_object::<WlSurface>(surface_object_id) else {
        return false;
    };
    is_managed_root_surface_candidate_common(state, surface_object_id, surface)
        && surface.has_committed_buffer()
}

fn is_managed_root_surface_candidate_common(
    state: &State,
    surface_object_id: ObjectId,
    surface: &WlSurface,
) -> bool {
    surface.is_root()
        && surface.role() == Role::XdgSurface
        && !surface.has_cursor_role()
        && !surface.has_dnd_icon_role()
        && xdg_toplevel_for_xdg_surface(state, surface_object_id, surface.xdg_surface_id())
            .is_some()
}

pub fn xdg_toplevel_states_for_edges(edges: u32) -> Vec<u32> {
    let mut states = Vec::new();
    if edges & EDGE_LEFT != 0 {
        states.push(xdg_toplevel::State::TiledLeft as u32);
    }
    if edges & EDGE_RIGHT != 0 {
        states.push(xdg_toplevel::State::TiledRight as u32);
    }
    if edges & EDGE_TOP != 0 {
        states.push(xdg_toplevel::State::TiledTop as u32);
    }
    if edges & EDGE_BOTTOM != 0 {
        states.push(xdg_toplevel::State::TiledBottom as u32);
    }
    states
}

#[cfg(test)]
pub(crate) fn test_tiled_resize_state() -> TiledResizeState {
    TiledResizeState {
        axis: SplitAxis::Horizontal,
        leaf_index: 0,
        neighbor_index: 1,
        pair_weight: 1.0,
        initial_leaf_weight: 0.5,
        axis_span: 100,
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{Client, ClientId},
            egl::EGL,
            globals::Globals,
            protocol_generated::{WlSurfaceId, XdgSurfaceId, XdgToplevelId, XdgWmBaseId},
            runner::Dispatcher,
            state::State,
            wayland_objects::{
                wl_surface::Role,
                xdg_surface::{xdg_toplevel::ToplevelDecorationMode, XdgSurface},
            },
            xkb::Xkb,
        },
    };

    fn make_state() -> State {
        let xkb = Xkb::new();
        let keyboard = xkb.create_keyboard_state();
        let mut globals = Globals::new();
        globals.initialize();
        State {
            objects: Default::default(),
            root_surfaces: Default::default(),
            layer_surface_objects: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            workspace_groups: Default::default(),
            workspaces: Default::default(),
            output_workspace_groups: Default::default(),
            root_surface_workspaces: Default::default(),
            managed_root_surfaces: Default::default(),
            xdg_toplevel_roles: Default::default(),
            next_workspace_serial: 1,
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            config: Default::default(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn insert_root_xdg_toplevel(
        state: &mut State,
        client: &mut Client,
        dispatcher: &mut crate::clients::ClientDispatcher<'_>,
        wl_surface_id: WlSurfaceId,
        xdg_surface_id: XdgSurfaceId,
        toplevel_id: XdgToplevelId,
        logical_size: (i32, i32),
    ) -> (ObjectId, ObjectId, ObjectId) {
        let ((_, surface_object_id), _) =
            client.insert_object(state, WlSurface::new(wl_surface_id, client.client_id(), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(surface.set_role(Role::XdgSurface, dispatcher).is_ok());
            surface.set_xdg_surface_impl(xdg_surface_id);
            surface.test_map(ObjectId::from(900usize), 0, 0, logical_size.0, logical_size.1);
        }
        state.register_root_surface(surface_object_id);

        let ((_, xdg_surface_object_id), _) = client.insert_object(
            state,
            XdgSurface::new(xdg_surface_id, XdgWmBaseId::from(1), wl_surface_id, 7),
        );
        let ((_, toplevel_object_id), _) =
            client.insert_object(state, XdgToplevel::new(toplevel_id, xdg_surface_id, 7));
        state.register_xdg_toplevel_role(client.client_id(), xdg_surface_id, toplevel_object_id);
        (surface_object_id, xdg_surface_object_id, toplevel_object_id)
    }

    #[test]
    fn tiled_resize_uses_grab_start_weights() {
        let mut layout = WorkspaceLayout::default();
        let left = ObjectId::from(1usize);
        let right = ObjectId::from(2usize);
        assert!(layout.insert_tiled(left));
        assert!(layout.insert_tiled(right));

        let bounds = RectI { x: 0, y: 0, width: 100, height: 50 };
        apply_workspace_layout(
            &mut layout,
            bounds,
            bounds,
            GapPolicy::default(),
            &BTreeMap::default(),
            &BTreeMap::default(),
        );

        let resize_state = layout
            .begin_tiled_resize(left, EDGE_RIGHT)
            .expect("left leaf should support right-edge resize");
        assert!(layout.resize_tiled_surface_from_state(left, resize_state, 10, 0));
        let first_left_weight = match layout.nodes.get(&layout.root.unwrap()).unwrap() {
            LayoutNode::Container(container) => container.children[0].weight,
            LayoutNode::Leaf(_) => panic!("root should be a container"),
        };

        assert!(layout.resize_tiled_surface_from_state(left, resize_state, 10, 0));
        let second_left_weight = match layout.nodes.get(&layout.root.unwrap()).unwrap() {
            LayoutNode::Container(container) => container.children[0].weight,
            LayoutNode::Leaf(_) => panic!("root should be a container"),
        };

        assert_eq!(first_left_weight, second_left_weight);
    }

    #[test]
    fn fullscreen_layout_applies_to_floating_surface() {
        let mut layout = WorkspaceLayout::default();
        let floating = ObjectId::from(3usize);
        layout.floating.stack.push(floating);
        assert!(layout.set_fullscreen(Some(floating)));

        let bounds = RectI { x: 10, y: 20, width: 300, height: 200 };
        let operations = apply_workspace_layout(
            &mut layout,
            bounds,
            bounds,
            GapPolicy::default(),
            &BTreeMap::default(),
            &BTreeMap::default(),
        );

        assert_eq!(
            operations,
            vec![LayoutOperation {
                surface_object_id: floating,
                frame_rect: bounds,
                content_rect: bounds,
            }]
        );
    }

    #[test]
    fn inserting_after_focus_can_create_vertical_subsplit() {
        let mut layout = WorkspaceLayout::default();
        let first = ObjectId::from(4usize);
        let second = ObjectId::from(5usize);
        let third = ObjectId::from(6usize);

        assert!(layout.insert_tiled(first));
        assert!(layout.insert_tiled(second));
        assert!(layout.insert_tiled(third));

        let root_id = layout.root.expect("root should exist");
        let root = layout
            .nodes
            .get(&root_id)
            .and_then(LayoutNode::as_container)
            .unwrap();
        assert_eq!(root.axis, SplitAxis::Horizontal);
        assert_eq!(root.children.len(), 2);

        let nested_id = root.children[1].node_id;
        let nested = layout
            .nodes
            .get(&nested_id)
            .and_then(LayoutNode::as_container)
            .unwrap();
        assert_eq!(nested.axis, SplitAxis::Vertical);
        assert_eq!(nested.children.len(), 2);
    }

    #[test]
    fn removing_focused_leaf_selects_remaining_tiled_focus() {
        let mut layout = WorkspaceLayout::default();
        let first = ObjectId::from(7usize);
        let second = ObjectId::from(8usize);

        assert!(layout.insert_tiled(first));
        assert!(layout.insert_tiled(second));
        assert!(layout.remove_surface(second));

        let focused_leaf = layout
            .focused_leaf
            .expect("remaining tile should stay focused");
        let focused_surface = layout
            .nodes
            .get(&focused_leaf)
            .and_then(LayoutNode::as_leaf)
            .map(|leaf| leaf.surface_object_id);
        assert_eq!(focused_surface, Some(first));
    }

    #[test]
    fn removing_last_leaf_from_nested_container_cleans_up_empty_ancestors() {
        let mut layout = WorkspaceLayout::default();
        let leaf_surface = ObjectId::from(9usize);
        let leaf_id = layout.alloc_leaf(leaf_surface, None);
        let inner_id = layout.alloc_container(
            SplitAxis::Vertical,
            None,
            vec![LayoutChild { node_id: leaf_id, weight: 1.0 }],
        );
        if let Some(node) = layout.nodes.get_mut(&leaf_id) {
            node.set_parent(Some(inner_id));
        }
        let root_id = layout.alloc_container(
            SplitAxis::Horizontal,
            None,
            vec![LayoutChild { node_id: inner_id, weight: 1.0 }],
        );
        if let Some(node) = layout.nodes.get_mut(&inner_id) {
            node.set_parent(Some(root_id));
        }
        layout.root = Some(root_id);

        assert!(layout.remove_surface(leaf_surface));
        assert_eq!(layout.root, None);
        assert!(layout.nodes.is_empty());
    }

    #[test]
    fn layout_handles_minimums_larger_than_available_space() {
        let mut layout = WorkspaceLayout::default();
        let left = ObjectId::from(10usize);
        let right = ObjectId::from(11usize);
        assert!(layout.insert_tiled(left));
        assert!(layout.insert_tiled(right));

        let bounds = RectI { x: 0, y: 0, width: 5, height: 20 };
        let min_sizes = BTreeMap::from([(left, (10, 1)), (right, (10, 1))]);
        let operations = apply_workspace_layout(
            &mut layout,
            bounds,
            bounds,
            GapPolicy::default(),
            &BTreeMap::default(),
            &min_sizes,
        );

        assert_eq!(operations.len(), 2);
        let total_width = operations
            .iter()
            .map(|operation| operation.frame_rect.width)
            .sum::<i32>();
        assert_eq!(total_width, bounds.width);
    }

    #[test]
    fn outer_gap_shrinks_workspace_tiling_bounds() {
        let usable_bounds = RectI { x: 10, y: 20, width: 200, height: 100 };
        let bounds = outer_gapped_bounds(usable_bounds, GapPolicy { outer_gap: 8, inner_gap: 0 });

        assert_eq!(bounds, RectI { x: 18, y: 28, width: 184, height: 84 });
    }

    #[test]
    fn inner_gap_inserts_spacing_between_sibling_tiles() {
        let mut layout = WorkspaceLayout::default();
        let left = ObjectId::from(12usize);
        let right = ObjectId::from(13usize);
        assert!(layout.insert_tiled(left));
        assert!(layout.insert_tiled(right));

        let operations = apply_workspace_layout(
            &mut layout,
            RectI { x: 0, y: 0, width: 100, height: 40 },
            RectI { x: 0, y: 0, width: 100, height: 40 },
            GapPolicy { outer_gap: 0, inner_gap: 10 },
            &BTreeMap::default(),
            &BTreeMap::default(),
        );

        assert_eq!(operations.len(), 2);
        assert_eq!(operations[0].frame_rect.x2() + 10, operations[1].frame_rect.x);
    }

    #[test]
    fn tiled_edges_are_relative_to_gapped_tiling_bounds() {
        let mut layout = WorkspaceLayout::default();
        let left = ObjectId::from(14usize);
        let right = ObjectId::from(15usize);
        assert!(layout.insert_tiled(left));
        assert!(layout.insert_tiled(right));

        let operations = apply_workspace_layout(
            &mut layout,
            RectI { x: 5, y: 7, width: 90, height: 30 },
            RectI { x: 0, y: 0, width: 100, height: 40 },
            GapPolicy::default(),
            &BTreeMap::default(),
            &BTreeMap::default(),
        );

        let left_leaf_id = layout.surface_leaves[&left];
        let right_leaf_id = layout.surface_leaves[&right];
        let left_edges = layout.nodes[&left_leaf_id].as_leaf().unwrap().tiled_edges;
        let right_edges = layout.nodes[&right_leaf_id].as_leaf().unwrap().tiled_edges;

        assert_eq!(operations[0].frame_rect.y, 7);
        assert!(left_edges & EDGE_LEFT != 0);
        assert!(left_edges & EDGE_RIGHT != 0);
        assert!(left_edges & EDGE_TOP != 0);
        assert!(left_edges & EDGE_BOTTOM != 0);
        assert!(right_edges & EDGE_LEFT != 0);
        assert!(right_edges & EDGE_RIGHT != 0);
        assert!(right_edges & EDGE_TOP != 0);
        assert!(right_edges & EDGE_BOTTOM != 0);
    }

    #[test]
    fn decoration_insets_shrink_tiled_content_rect() {
        let frame = RectI { x: 10, y: 20, width: 100, height: 80 };
        let content = content_rect_for_tiled_frame(
            frame,
            DecorationInsets { top: 3, right: 4, bottom: 5, left: 6 },
        );

        assert_eq!(content, RectI { x: 16, y: 23, width: 90, height: 72 });
    }

    #[test]
    fn window_geometry_derives_per_edge_decoration_insets() {
        let surface = WlSurface::new(WlSurfaceId::from(30u32), ClientId::from_raw(31), 6);
        let mut state = make_state();
        let ((_, surface_object_id), _) =
            Client::new_for_tests(ClientId::from_raw(32)).insert_object(&mut state, surface);
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            surface.test_map(ObjectId::from(901usize), 0, 0, 80, 60);
            let insets = decoration_insets_from_window_geometry(
                &surface,
                RectI { x: -4, y: -12, width: 90, height: 80 },
            )
            .unwrap();
            assert_eq!(insets, DecorationInsets { top: 12, right: 6, bottom: 8, left: 4 });
        }
    }

    #[test]
    fn window_geometry_positive_origin_derives_csd_title_bar_insets() {
        let insets = decoration_insets_from_window_geometry_and_logical_size(
            SizeI { width: 800, height: 600 },
            RectI { x: 0, y: 32, width: 800, height: 568 },
        )
        .unwrap();
        assert_eq!(insets, DecorationInsets { top: 32, right: 0, bottom: 0, left: 0 });
    }

    #[test]
    fn tiled_root_position_aligns_negative_origin_geometry_to_frame_rect() {
        let mut state = make_state();
        let mut client = Client::new_for_tests(ClientId::from_raw(44));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let (surface_object_id, _xdg_surface_object_id, toplevel_object_id) =
            insert_root_xdg_toplevel(
                &mut state,
                &mut client,
                &mut client_dispatcher,
                WlSurfaceId::from(45u32),
                XdgSurfaceId::from(46u32),
                XdgToplevelId::from(47u32),
                (80, 60),
            );

        let ((_, child_surface_object_id), _) = client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(48u32), client.client_id(), 6),
        );
        {
            let root_surface = state
                .objects
                .get_mut(surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.add_pending_child(child_surface_object_id);
        }
        {
            let child_surface = state
                .objects
                .get_mut(child_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(49u32),
                surface_object_id,
            );
            child_surface.set_subsurface_position(-4, -12);
            child_surface.test_map(ObjectId::from(901usize), 0, 0, 90, 80);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }
        {
            let toplevel = state
                .objects
                .get_mut(toplevel_object_id)
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            toplevel.set_window_geometry(Some(RectI { x: -4, y: -12, width: 90, height: 80 }));
        }

        let surface = state
            .objects
            .try_ref_object::<WlSurface>(surface_object_id)
            .unwrap();
        let frame_rect = RectI { x: 10, y: 20, width: 90, height: 80 };
        let content_rect = RectI { x: 14, y: 32, width: 80, height: 60 };

        assert_eq!(
            tiled_root_position_for_layout(
                &state,
                surface_object_id,
                surface,
                frame_rect,
                content_rect,
            ),
            (14, 32)
        );
    }

    #[test]
    fn tiled_root_position_aligns_positive_origin_geometry_to_frame_rect() {
        let mut state = make_state();
        let mut client = Client::new_for_tests(ClientId::from_raw(60));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let (surface_object_id, _xdg_surface_object_id, toplevel_object_id) =
            insert_root_xdg_toplevel(
                &mut state,
                &mut client,
                &mut client_dispatcher,
                WlSurfaceId::from(61u32),
                XdgSurfaceId::from(62u32),
                XdgToplevelId::from(63u32),
                (800, 600),
            );
        {
            let toplevel = state
                .objects
                .get_mut(toplevel_object_id)
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            toplevel.set_window_geometry(Some(RectI { x: 0, y: 32, width: 800, height: 568 }));
        }

        let surface = state
            .objects
            .try_ref_object::<WlSurface>(surface_object_id)
            .unwrap();
        let frame_rect = RectI { x: 10, y: 20, width: 800, height: 568 };
        let content_rect = RectI { x: 10, y: 52, width: 800, height: 536 };

        assert_eq!(
            tiled_root_position_for_layout(
                &state,
                surface_object_id,
                surface,
                frame_rect,
                content_rect,
            ),
            (10, -12)
        );
    }

    #[test]
    fn effective_decoration_insets_fall_back_to_config_without_explicit_geometry() {
        let mut state = make_state();
        state.config.general.decoration_insets =
            DecorationInsets { top: 7, right: 1, bottom: 2, left: 3 };
        let mut client = Client::new_for_tests(ClientId::from_raw(40));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let (surface_object_id, _xdg_surface_object_id, _toplevel_object_id) =
            insert_root_xdg_toplevel(
                &mut state,
                &mut client,
                &mut client_dispatcher,
                WlSurfaceId::from(41u32),
                XdgSurfaceId::from(42u32),
                XdgToplevelId::from(43u32),
                (80, 60),
            );

        assert_eq!(
            effective_decoration_insets(&state, surface_object_id),
            DecorationInsets { top: 7, right: 1, bottom: 2, left: 3 }
        );
    }

    #[test]
    fn effective_decoration_insets_use_explicit_window_geometry_for_csd() {
        let mut state = make_state();
        state.config.general.decoration_insets =
            DecorationInsets { top: 1, right: 1, bottom: 1, left: 1 };
        let mut client = Client::new_for_tests(ClientId::from_raw(50));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let (surface_object_id, _xdg_surface_object_id, toplevel_object_id) =
            insert_root_xdg_toplevel(
                &mut state,
                &mut client,
                &mut client_dispatcher,
                WlSurfaceId::from(51u32),
                XdgSurfaceId::from(52u32),
                XdgToplevelId::from(53u32),
                (80, 60),
            );
        let ((_, child_surface_object_id), _) = client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(54u32), client.client_id(), 6),
        );
        {
            let root_surface = state
                .objects
                .get_mut(surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.add_pending_child(child_surface_object_id);
        }
        {
            let child_surface = state
                .objects
                .get_mut(child_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(55u32),
                surface_object_id,
            );
            child_surface.set_subsurface_position(-4, -12);
            child_surface.test_map(ObjectId::from(901usize), 0, 0, 90, 80);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }
        {
            let toplevel = state
                .objects
                .get_mut(toplevel_object_id)
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            toplevel.set_window_geometry(Some(RectI { x: -4, y: -12, width: 90, height: 80 }));
        }

        assert_eq!(
            effective_decoration_insets(&state, surface_object_id),
            DecorationInsets { top: 12, right: 6, bottom: 8, left: 4 }
        );
    }

    #[test]
    fn effective_decoration_insets_clamp_window_geometry_to_surface_tree_bounds() {
        let mut state = make_state();
        state.config.general.decoration_insets =
            DecorationInsets { top: 1, right: 1, bottom: 1, left: 1 };
        let mut client = Client::new_for_tests(ClientId::from_raw(55));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let (surface_object_id, _xdg_surface_object_id, toplevel_object_id) =
            insert_root_xdg_toplevel(
                &mut state,
                &mut client,
                &mut client_dispatcher,
                WlSurfaceId::from(56u32),
                XdgSurfaceId::from(57u32),
                XdgToplevelId::from(58u32),
                (80, 60),
            );
        let ((_, child_surface_object_id), _) = client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(59u32), client.client_id(), 6),
        );
        {
            let root_surface = state
                .objects
                .get_mut(surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.add_pending_child(child_surface_object_id);
        }
        {
            let child_surface = state
                .objects
                .get_mut(child_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(60u32),
                surface_object_id,
            );
            child_surface.set_subsurface_position(-4, -12);
            child_surface.test_map(ObjectId::from(902usize), 0, 0, 90, 80);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }
        {
            let toplevel = state
                .objects
                .get_mut(toplevel_object_id)
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            toplevel.set_window_geometry(Some(RectI { x: -10, y: -20, width: 120, height: 100 }));
        }

        assert_eq!(
            effective_decoration_insets(&state, surface_object_id),
            DecorationInsets { top: 12, right: 6, bottom: 8, left: 4 }
        );
    }

    #[test]
    fn server_side_decoration_mode_uses_configured_fallback_insets() {
        let mut state = make_state();
        state.config.general.decoration_insets =
            DecorationInsets { top: 2, right: 3, bottom: 4, left: 5 };
        let mut client = Client::new_for_tests(ClientId::from_raw(60));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let (surface_object_id, _xdg_surface_object_id, toplevel_object_id) =
            insert_root_xdg_toplevel(
                &mut state,
                &mut client,
                &mut client_dispatcher,
                WlSurfaceId::from(61u32),
                XdgSurfaceId::from(62u32),
                XdgToplevelId::from(63u32),
                (80, 60),
            );
        {
            let toplevel = state
                .objects
                .get_mut(toplevel_object_id)
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            toplevel.set_window_geometry(Some(RectI { x: -7, y: -9, width: 94, height: 74 }));
        }
        state
            .objects
            .get_mut(toplevel_object_id)
            .downcast_mut::<XdgToplevel>()
            .unwrap()
            .test_set_decoration_mode(ToplevelDecorationMode::ServerSide);

        assert_eq!(
            effective_decoration_insets(&state, surface_object_id),
            DecorationInsets { top: 2, right: 3, bottom: 4, left: 5 }
        );
    }

    #[test]
    fn fullscreen_layout_ignores_gaps_and_insets() {
        let mut layout = WorkspaceLayout::default();
        let surface = ObjectId::from(16usize);
        layout.floating.stack.push(surface);
        assert!(layout.set_fullscreen(Some(surface)));

        let fullscreen_bounds = RectI { x: 3, y: 4, width: 120, height: 70 };
        let operations = apply_workspace_layout(
            &mut layout,
            RectI { x: 20, y: 30, width: 80, height: 20 },
            fullscreen_bounds,
            GapPolicy { outer_gap: 10, inner_gap: 7 },
            &BTreeMap::from([(surface, DecorationInsets { top: 2, right: 2, bottom: 2, left: 2 })]),
            &BTreeMap::default(),
        );

        assert_eq!(operations[0].frame_rect, fullscreen_bounds);
        assert_eq!(operations[0].content_rect, fullscreen_bounds);
    }

    #[test]
    fn mixed_decoration_insets_shrink_each_leaf_independently() {
        let mut layout = WorkspaceLayout::default();
        let left = ObjectId::from(160usize);
        let right = ObjectId::from(161usize);
        assert!(layout.insert_tiled(left));
        assert!(layout.insert_tiled(right));

        let operations = apply_workspace_layout(
            &mut layout,
            RectI { x: 0, y: 0, width: 120, height: 80 },
            RectI { x: 0, y: 0, width: 120, height: 80 },
            GapPolicy::default(),
            &BTreeMap::from([
                (left, DecorationInsets { top: 10, right: 0, bottom: 0, left: 0 }),
                (right, DecorationInsets::default()),
            ]),
            &BTreeMap::default(),
        );

        assert_eq!(operations.len(), 2);
        assert_eq!(operations[0].content_rect.y, 10);
        assert_eq!(operations[1].content_rect.y, 0);
    }

    #[test]
    fn minimum_size_fallback_accounts_for_inner_gaps() {
        let mut layout = WorkspaceLayout::default();
        let left = ObjectId::from(17usize);
        let right = ObjectId::from(18usize);
        assert!(layout.insert_tiled(left));
        assert!(layout.insert_tiled(right));

        let operations = apply_workspace_layout(
            &mut layout,
            RectI { x: 0, y: 0, width: 8, height: 20 },
            RectI { x: 0, y: 0, width: 8, height: 20 },
            GapPolicy { outer_gap: 0, inner_gap: 4 },
            &BTreeMap::default(),
            &BTreeMap::from([(left, (10, 1)), (right, (10, 1))]),
        );

        let total_width = operations
            .iter()
            .map(|operation| operation.frame_rect.width)
            .sum::<i32>();
        let gap = operations[1].frame_rect.x - operations[0].frame_rect.x2();
        assert_eq!(total_width + gap, 8);
    }

    #[test]
    fn node_minimum_size_uses_per_surface_decoration_insets() {
        let mut layout = WorkspaceLayout::default();
        let left = ObjectId::from(170usize);
        let right = ObjectId::from(171usize);
        assert!(layout.insert_tiled(left));
        assert!(layout.insert_tiled(right));

        let minimum = node_minimum_size(
            &layout,
            layout.root.expect("root should exist"),
            None,
            GapPolicy::default(),
            &BTreeMap::from([
                (left, DecorationInsets { top: 0, right: 4, bottom: 0, left: 2 }),
                (right, DecorationInsets { top: 0, right: 1, bottom: 0, left: 1 }),
            ]),
            &BTreeMap::from([(left, (10, 1)), (right, (10, 1))]),
        );

        assert_eq!(minimum, (28, 1));
    }

    #[test]
    fn nested_container_minimum_uses_effective_gap_from_size_hint() {
        let mut layout = WorkspaceLayout::default();
        let first = ObjectId::from(19usize);
        let second = ObjectId::from(20usize);
        let third = ObjectId::from(21usize);
        let first_id = layout.alloc_leaf(first, None);
        let second_id = layout.alloc_leaf(second, None);
        let third_id = layout.alloc_leaf(third, None);
        let container_id = layout.alloc_container(
            SplitAxis::Horizontal,
            None,
            vec![
                LayoutChild { node_id: first_id, weight: 1.0 },
                LayoutChild { node_id: second_id, weight: 1.0 },
                LayoutChild { node_id: third_id, weight: 1.0 },
            ],
        );
        for child_id in [first_id, second_id, third_id] {
            if let Some(node) = layout.nodes.get_mut(&child_id) {
                node.set_parent(Some(container_id));
            }
        }

        let minimum = node_minimum_size(
            &layout,
            container_id,
            Some(LayoutSizeHint { width: 5, height: 30 }),
            GapPolicy { outer_gap: 0, inner_gap: 10 },
            &BTreeMap::default(),
            &BTreeMap::default(),
        );

        assert_eq!(minimum, (7, 1));
    }

    #[test]
    fn nested_splits_propagate_internal_tiled_edge_constraints() {
        let mut layout = WorkspaceLayout::default();
        let first = ObjectId::from(22usize);
        let second = ObjectId::from(23usize);
        let third = ObjectId::from(24usize);
        assert!(layout.insert_tiled(first));
        assert!(layout.insert_tiled(second));
        assert!(layout.insert_tiled(third));

        apply_workspace_layout(
            &mut layout,
            RectI { x: 0, y: 0, width: 120, height: 80 },
            RectI { x: 0, y: 0, width: 120, height: 80 },
            GapPolicy::default(),
            &BTreeMap::default(),
            &BTreeMap::default(),
        );

        let first_edges = layout.nodes[&layout.surface_leaves[&first]]
            .as_leaf()
            .unwrap()
            .tiled_edges;
        let second_edges = layout.nodes[&layout.surface_leaves[&second]]
            .as_leaf()
            .unwrap()
            .tiled_edges;
        let third_edges = layout.nodes[&layout.surface_leaves[&third]]
            .as_leaf()
            .unwrap()
            .tiled_edges;

        assert_eq!(first_edges, EDGE_LEFT | EDGE_RIGHT | EDGE_TOP | EDGE_BOTTOM);
        assert_eq!(second_edges, EDGE_LEFT | EDGE_RIGHT | EDGE_TOP | EDGE_BOTTOM);
        assert_eq!(third_edges, EDGE_LEFT | EDGE_RIGHT | EDGE_TOP | EDGE_BOTTOM);
    }
}

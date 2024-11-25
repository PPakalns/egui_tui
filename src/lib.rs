#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use egui::util::IdTypeMap;
use egui::{Pos2, Response, Ui};
use taffy::prelude::*;
use widgets::TaffySeparator;

////////////////////////////////////////////////////////////////////////////////

pub use taffy;

/// Widgets built combining multiple taffy nodes
pub mod widgets;

/// Implemets functionality for egui widgets to be used as taffy leaf containers
mod egui_widgets;

/// Helper function to initialize taffy layout
pub fn tui(ui: &mut egui::Ui, id: impl Into<egui::Id>) -> TuiInitializer<'_> {
    TuiInitializer {
        ui,
        id: id.into(),
        allocated_rect: None,
        available_space: Size {
            width: AvailableSpace::MinContent,
            height: AvailableSpace::MinContent,
        },
        style: Default::default(),
        known_size: Size {
            width: None,
            height: None,
        },
    }
}

/// Egui tui initialization helper to reserve/allocate necessary space
#[must_use]
pub struct TuiInitializer<'a> {
    ui: &'a mut egui::Ui,
    allocated_rect: Option<egui::Rect>,
    available_space: Size<AvailableSpace>,
    known_size: Size<Option<f32>>,
    style: taffy::Style,
    id: egui::Id,
}

impl<'a> TuiInitializer<'a> {
    /// Place ui in already allocated rectangle
    pub fn with_allocated_rect(mut self, rect: egui::Rect) -> TuiInitializer<'a> {
        self.allocated_rect = Some(rect);
        self.available_space = Size {
            width: AvailableSpace::Definite(rect.width()),
            height: AvailableSpace::Definite(rect.height()),
        };
        self
    }

    /// Reserve space
    pub fn reserve_space(self, space: egui::Vec2) -> TuiInitializer<'a> {
        self.reserve_width(space.x).reserve_height(space.y)
    }

    /// Reserve specific width for tui
    pub fn reserve_width(mut self, width: f32) -> TuiInitializer<'a> {
        self.ui.set_min_width(width);
        self.available_space.width = AvailableSpace::Definite(width);
        self.known_size.width = Some(width);
        self
    }

    /// Reserve specific height for tui
    pub fn reserve_height(mut self, height: f32) -> TuiInitializer<'a> {
        self.ui.set_min_height(height);
        self.available_space.height = AvailableSpace::Definite(height);
        self.known_size.height = Some(height);
        self
    }

    /// Reserve all available width
    pub fn reserve_available_space(self) -> TuiInitializer<'a> {
        self.reserve_available_width().reserve_available_height()
    }

    /// Reserve all available width
    pub fn reserve_available_width(self) -> TuiInitializer<'a> {
        let width = self.ui.available_size().x;
        self.reserve_width(width)
    }

    /// Reserve all available height
    pub fn reserve_available_height(self) -> TuiInitializer<'a> {
        let height = self.ui.available_size().y;
        self.reserve_height(height)
    }

    /// Set custom sizing constraints for taffy layouting algorithm for available space
    pub fn with_available_space(
        mut self,
        available_space: Size<AvailableSpace>,
    ) -> TuiInitializer<'a> {
        self.available_space = available_space;
        self
    }

    /// Set root container style
    pub fn style(mut self, style: taffy::Style) -> TuiInitializer<'a> {
        self.style = style;
        self
    }

    /// Show tui
    pub fn show<T>(self, f: impl FnOnce(&mut Tui<'_>) -> T) -> T {
        let ui = self.ui;
        let output = Tui::create(
            ui,
            self.id,
            ui.available_rect_before_wrap(),
            Some(self.available_space),
            self.style,
            |tui| {
                // Temporary scroll area size limitation
                tui.set_limit_scroll_area_size(Some(0.7));

                f(tui)
            },
        );

        if self.allocated_rect.is_none() {
            // Space was not allocated yet, allocate used space
            let size = output.container.layout.content_size;
            ui.allocate_space(egui::Vec2 {
                x: size.width,
                y: size.height,
            });
        }
        output.inner
    }
}

/// Tui (Egui Taffy UI) is used to place ui nodes and set their id, style configuration
pub struct Tui<'a> {
    main_id: egui::Id,

    ui: &'a mut Ui,

    current_id: egui::Id,
    current_node: Option<NodeId>,
    current_node_index: usize,
    last_child_count: usize,
    parent_rect: egui::Rect,

    used_items: HashSet<egui::Id>,

    root_rect: egui::Rect,
    available_space: Option<Size<AvailableSpace>>,

    /// Temporary default limit on scroll area size due to taffy
    /// being unable to shrink container to be smaller than content automatically
    limit_scroll_area_size: Option<f32>,
}

impl<'a> Tui<'a> {
    /// Retrieve stored layout information in egui memory
    fn with_state<T>(id: egui::Id, ctx: egui::Context, f: impl FnOnce(&mut TaffyState) -> T) -> T {
        let state = ctx.data_mut(|data: &mut IdTypeMap| {
            let state: Arc<Mutex<TaffyState>> = data
                .get_temp_mut_or_insert_with(id, || Arc::new(Mutex::new(TaffyState::new())))
                .clone();
            state
        });

        let mut state = state.lock().unwrap();

        f(&mut state)
    }

    /// Manually create Tui, user must manually allocate space in egui Ui if this method is used
    /// directly instead of helper method [`tui`]
    pub fn create<T>(
        ui: &'a mut Ui,
        id: egui::Id,
        root_rect: egui::Rect,
        available_space: Option<Size<AvailableSpace>>,
        style: Style,
        f: impl FnOnce(&mut Tui<'_>) -> T,
    ) -> TaffyReturn<T> {
        let mut this = Self {
            main_id: id,
            ui,
            current_node: None,
            current_node_index: 0,
            last_child_count: 0,
            parent_rect: root_rect,
            used_items: Default::default(),
            root_rect,
            available_space,
            current_id: id,
            limit_scroll_area_size: None,
        };

        this.tui().id(id).style(style).add(|state| {
            let resp = f(state);
            let container = state.recalculate();
            TaffyReturn {
                inner: resp,
                container,
            }
        })
    }

    /// Set maximal size coefficient of scroll area based on root element size
    ///
    /// `scroll_area max height = root_height * size`
    ///
    /// Taffy doesn't correctly shrink nodes that should have larger content than their size
    /// (overflow)
    pub fn set_limit_scroll_area_size(&mut self, size: Option<f32>) {
        self.limit_scroll_area_size = size;
    }

    /// Add taffy child node, correctly update taffy tree state
    fn add_child_node(&mut self, id: egui::Id, style: taffy::Style) -> (NodeId, TaffyContainerUi) {
        if self.used_items.contains(&id) {
            log::error!("Taffy layout id collision!");
        }

        Self::with_state(self.main_id, self.ui.ctx().clone(), |state| {
            let child_idx = self.current_node_index;
            self.current_node_index += 1;

            self.used_items.insert(id);
            let mut first_frame = false;

            let node_id = if let Some(node_id) = state.items.get(&id).copied() {
                if state.taffy.style(node_id).unwrap() != &style {
                    state.taffy.set_style(node_id, style).unwrap();
                }
                node_id
            } else {
                first_frame = true;
                let node = state.taffy.new_leaf(style).unwrap();
                state.items.insert(id, node);
                node
            };

            if let Some(current_node) = self.current_node {
                if child_idx < self.last_child_count {
                    if state.taffy.child_at_index(current_node, child_idx).unwrap() != node_id {
                        state
                            .taffy
                            .replace_child_at_index(current_node, child_idx, node_id)
                            .unwrap();
                    }
                } else {
                    state.taffy.add_child(current_node, node_id).unwrap();
                    self.last_child_count += 1;
                }
            }

            let container = TaffyContainerUi {
                layout: state.layout(node_id),
                parent_rect: self.parent_rect,
                first_frame,
            };

            (node_id, container)
        })
    }

    /// Add child taffy node to the layout with optional function to draw background
    fn add_children_inner<'r, T>(
        &'r mut self,
        id: TuiId,
        style: Style,
        content: Option<impl FnOnce(&mut egui::Ui)>,
        f: impl FnOnce(&mut Tui<'a>) -> T,
    ) -> T {
        let id = id.resolve(self);

        let (node_id, render_options) = self.add_child_node(id, style);

        let stored_id = self.current_id;
        let stored_node = self.current_node;
        let stored_node_index = self.current_node_index;
        let stored_last_child_count = self.last_child_count;
        let stored_parent_rect = self.parent_rect;

        Self::with_state(self.main_id, self.ui.ctx().clone(), |state| {
            self.current_node = Some(node_id);
            self.current_node_index = 0;
            self.last_child_count = state.taffy.child_count(node_id);

            let max_rect = render_options.full_container();
            self.parent_rect = if max_rect.any_nan() {
                self.parent_rect
            } else {
                max_rect
            };
            self.current_id = id;
        });

        if let Some(content) = content {
            let max_rect = render_options.full_container();
            if !max_rect.any_nan() {
                let mut child_ui = self.ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(id.with("background"))
                        .max_rect(max_rect),
                );
                content(&mut child_ui);
            }
        }
        let resp = f(self);

        Self::with_state(self.main_id, self.ui.ctx().clone(), |state| {
            let mut current_cnt = state.taffy.child_count(node_id);

            while current_cnt > self.last_child_count {
                state
                    .taffy
                    .remove_child_at_index(node_id, current_cnt - 1)
                    .unwrap();
                current_cnt -= 1;
            }
        });

        self.current_id = stored_id;
        self.current_node = stored_node;
        self.current_node_index = stored_node_index;
        self.last_child_count = stored_last_child_count;
        self.parent_rect = stored_parent_rect;

        resp
    }

    /// Add egui user interface as child node in the Tui
    fn add_container<T>(
        &mut self,
        id: impl Into<TuiId>,
        style: taffy::Style,
        content: impl FnOnce(&mut Ui, TaffyContainerUi) -> TuiContainerResponse<T>,
    ) -> T {
        let id = id.into().resolve(self);

        let (nodeid, mut render_options) = self.add_child_node(id, style.clone());

        let mut ui_builder = egui::UiBuilder::new()
            .max_rect(render_options.inner_container())
            .id_salt(id.with("_ui"))
            .layout(Default::default());

        // Inner boxes are vertical by default
        ui_builder.layout.as_mut().unwrap().main_dir = egui::Direction::TopDown;

        // TODO: Handle correctly case where max_rect has NaN values
        if ui_builder.max_rect.unwrap().any_nan() {
            render_options.first_frame = true;
            ui_builder = ui_builder.max_rect(self.parent_rect);
        }

        if render_options.first_frame {
            ui_builder = ui_builder.sizing_pass().invisible();
        }

        let mut child_ui = self.ui.new_child(ui_builder);
        let resp = content(&mut child_ui, render_options);

        Self::with_state(self.main_id, self.ui.ctx().clone(), |state| {
            let min_size = if let Some(intrinsic_size) = resp.intrinsic_size {
                resp.min_size.min(intrinsic_size).ceil()
            } else {
                resp.min_size.ceil()
            };

            let mut max_size = resp.max_size;
            max_size = max_size.max(min_size);

            let new_content = Context {
                min_size,
                max_size,
                infinite: resp.infinite,
            };
            if state.taffy.get_node_context(nodeid) != Some(&new_content) {
                state
                    .taffy
                    .set_node_context(nodeid, Some(new_content))
                    .unwrap();
            }
        });
        resp.inner
    }

    /// Add scroll area node to the taffy layout
    ///
    /// TODO: Add support for scroll area content to be handled in the same taffy tree
    fn add_scroll_area_ext<T>(
        &mut self,
        id: impl Into<TuiId>,
        mut style: taffy::Style,
        limit: Option<f32>,
        content: impl FnOnce(&mut Ui) -> T,
    ) -> T {
        style.overflow = taffy::Point {
            x: taffy::Overflow::Visible,
            y: taffy::Overflow::Hidden,
        };
        style.display = taffy::Display::Block;
        style.min_size = Size {
            width: Dimension::Length(0.),
            height: Dimension::Length(0.),
        };
        if let Some(limit) = limit {
            style.max_size.height = Dimension::Length(self.root_rect.height() * limit);
            style.max_size.width = Dimension::Length(self.root_rect.width() * limit);
        }
        self.tui().id(id).style(style).add(|tui| {
            let layout = Self::with_state(tui.main_id, tui.ui.ctx().clone(), |state| {
                *state.taffy.layout(tui.current_node.unwrap()).unwrap()
            });

            let style = taffy::Style {
                ..Default::default()
            };

            tui.add_container("inner", style, |ui, _params| {
                let mut real_min_size = None;
                let scroll_area = egui::ScrollArea::both()
                    .id_salt(ui.id().with("scroll_area"))
                    .max_width(ui.available_width())
                    .min_scrolled_width(layout.size.width)
                    .max_width(layout.size.width)
                    .min_scrolled_height(layout.size.height)
                    .max_height(layout.size.height)
                    .show(ui, |ui| {
                        let resp = content(ui);
                        real_min_size = Some(ui.min_size());
                        resp
                    });

                let potential_frame_size = scroll_area.content_size;

                // let min_size = egui::Vec2 {
                //     x: potential_frame_size.x,
                //     y: 0.3 * potential_frame_size.y,
                // };
                let max_size = egui::Vec2 {
                    x: potential_frame_size.x,
                    y: potential_frame_size.y,
                };

                TuiContainerResponse {
                    inner: scroll_area.inner,
                    min_size: real_min_size.unwrap_or(max_size),
                    intrinsic_size: None,
                    max_size,
                    infinite: egui::Vec2b::FALSE,
                    scroll_area: true,
                }
            })
        })
    }

    /// Check if tui layout has changed, recalculate if necessary and trigger
    /// request discard for egui to redraw the UI
    fn recalculate(&mut self) -> TaffyContainerUi {
        let root_rect = self.root_rect;
        let available_space = self.available_space.unwrap_or(Size {
            width: AvailableSpace::Definite(root_rect.width()),
            height: AvailableSpace::Definite(root_rect.height()),
        });

        let current_node = self.current_node.unwrap();
        Self::with_state(self.main_id, self.ui.ctx().clone(), |state| {
            // Remove unused nodes (Removes unused child nodes too )
            state.items.retain(|k, v| {
                if self.used_items.contains(k) {
                    return true;
                }
                if let Some(parent) = state.taffy.parent(*v) {
                    state.taffy.remove_child(parent, *v).unwrap();
                }
                state.taffy.remove(*v).unwrap();
                false
            });
            self.used_items.clear();

            let taffy = &mut state.taffy;

            if taffy.dirty(current_node).unwrap() || state.last_size != root_rect.size() {
                // let ctx = self.ui.ctx();

                state.last_size = root_rect.size();
                taffy
                    .compute_layout_with_measure(
                        current_node,
                        available_space,
                        |_known_size: Size<Option<f32>>,
                         available_space: Size<AvailableSpace>,
                         _id,
                         context,
                         _style|
                         -> Size<f32> {
                            let context = context.copied().unwrap_or(Context {
                                min_size: egui::Vec2::ZERO,
                                max_size: egui::Vec2::ZERO,
                                infinite: egui::Vec2b::FALSE,
                            });

                            let Context {
                                mut min_size,
                                mut max_size,
                                infinite,
                            } = context;

                            // if scroll_area {
                            //     min_size = egui::Vec2::ZERO;
                            // }

                            if min_size.any_nan() {
                                min_size = egui::Vec2::ZERO;
                            }
                            if max_size.any_nan() {
                                max_size = root_rect.size();
                            }

                            let max_size = egui::Vec2 {
                                x: if infinite.x {
                                    root_rect.width()
                                } else {
                                    max_size.x
                                },
                                y: if infinite.y {
                                    root_rect.height()
                                } else {
                                    max_size.y
                                },
                            };

                            let width = match available_space.width {
                                AvailableSpace::Definite(num) => {
                                    num.clamp(min_size.x, max_size.x.max(min_size.x))
                                }
                                AvailableSpace::MinContent => min_size.x,
                                AvailableSpace::MaxContent => max_size.x,
                            };
                            let height = match available_space.height {
                                AvailableSpace::Definite(num) => {
                                    num.clamp(min_size.y, max_size.y.max(min_size.y))
                                }
                                AvailableSpace::MinContent => min_size.y,
                                AvailableSpace::MaxContent => max_size.y,
                            };

                            #[allow(clippy::let_and_return)]
                            let final_size = Size { width, height };

                            // println!(
                            //     "{:?} {:?} {:?} {:?} {:?} {:?}",
                            //     _id, min_size, max_size, available_space, final_size, _known_size,
                            // );

                            final_size
                        },
                    )
                    .unwrap();
                // taffy.print_tree(current_node);

                log::trace!("Taffy recalculation done!");
                self.ui.ctx().request_discard("Taffy recalculation");
            }

            TaffyContainerUi {
                parent_rect: root_rect,
                layout: state.layout(current_node),
                first_frame: false,
            }
        })
    }

    /// Access underlaying egui ui
    #[inline]
    pub fn egui_ui(&self) -> &&'a mut Ui {
        &self.ui
    }

    /// Initial root rect size set by the user
    ///
    /// (Used size in reality could change based on available space settings )
    pub fn root_rect(&self) -> egui::Rect {
        self.root_rect
    }

    /// Modify underlaying egui style
    #[inline]
    pub fn egui_style_mut(&mut self) -> &mut egui::Style {
        self.ui.style_mut()
    }
}

/// Tui returned information about final layout of the Tui
///
/// Can be used to allocate necessary space in parent egui::Ui
pub struct TaffyReturn<T> {
    /// Value returned by closure
    pub inner: T,
    /// Container layout information
    pub container: TaffyContainerUi,
}

/// Sizing context retrieved from Tui layout leaf nodes (egui widgets or child egui::Ui)
///
/// Used to calculate final layout in taffy layout calculations
#[derive(PartialEq, Default, Clone, Copy)]
struct Context {
    min_size: egui::Vec2,
    max_size: egui::Vec2,
    infinite: egui::Vec2b,
}

/// Helper to show the inner content of a container.
pub struct TaffyContainerUi {
    parent_rect: egui::Rect,
    layout: taffy::Layout,
    first_frame: bool,
}

impl TaffyContainerUi {
    /// Full container size
    pub fn full_container(&self) -> egui::Rect {
        let layout = &self.layout;
        let rect = egui::Rect::from_min_size(
            Pos2::new(layout.location.x, layout.location.y),
            egui::Vec2::new(layout.size.width, layout.size.height),
        );
        rect.translate(self.parent_rect.min.to_vec2())
    }

    /// Container from which padding has been removed
    pub fn inner_container(&self) -> egui::Rect {
        let layout = &self.layout;

        let size = layout.size
            - Size {
                width: layout.padding.left + layout.padding.right,
                height: layout.padding.top + layout.padding.bottom,
            };

        let rect = egui::Rect::from_min_size(
            Pos2::new(
                layout.location.x + layout.padding.left,
                layout.location.y + layout.padding.top,
            ),
            egui::Vec2::new(size.width, size.height),
        );
        rect.translate(self.parent_rect.min.to_vec2())
    }
}

/// Describes information about used space when laying out elements
///
/// This information is used for taffy layout calculation logic
pub struct TuiContainerResponse<T> {
    /// Closure return value
    pub inner: T,
    /// Minimal size that this widget can shrink to
    pub min_size: egui::Vec2,
    /// Minimal size reported by egui widget that this widget can shrink to
    pub intrinsic_size: Option<egui::Vec2>,
    /// Maximal size this widget can grow to
    ///
    /// See [`TuiContainerResponse::infinite`] to support containers that can grow infinitely
    pub max_size: egui::Vec2,
    /// Can widget grow to infinite size in given dimensions
    pub infinite: egui::Vec2b,
    /// Is container a scroll area
    pub scroll_area: bool,
}

////////////////////////////////////////////////////////////////////////////////

/// Implement this trait for a widget to make it usable in a tui container.
///
/// The reason there is a separate trait is that we need to measure the content size independently
/// of the frame size. (The content will stay at it's intrinsic size while the frame will be
/// stretched according to the flex layout.)
///
/// If your widget has no frame, you can use [`TuiBuilderLogic::ui``] directly to implement this trait.
///
/// See [`crate::widgets`] and `./egui_widgets.rs` for example trait implementations.
///
/// Trait idea taken from egui_flex
pub trait TuiWidget {
    /// The response type of the widget
    type Response;
    /// Show your widget here. Use the provided [`TuiBuilder`] to draw your widget correctly
    /// using the given style.
    fn taffy_ui(self, tuib: TuiBuilder) -> Self::Response;
}

////////////////////////////////////////////////////////////////////////////////

/// Id type to simplify defining layout node ids
#[derive(Default)]
pub enum TuiId {
    /// Create id based on parent node id and given id
    ///
    /// This is useful to avoid duplicated ids if the same layout is used in multiple places
    Hiarchy(egui::Id),

    /// Unique id, useful if node will be moved around in the layout and
    /// full layout recalculation is not needed
    Unique(egui::Id),

    /// Auto generate id using which child in the sequence this element is in parent node
    #[default]
    Auto,
}

impl TuiId {
    /// Calculate final id based on the current Tui state
    fn resolve(self, tui: &Tui) -> egui::Id {
        match self {
            TuiId::Hiarchy(id) => tui.current_id.with(id),
            TuiId::Unique(id) => id,
            TuiId::Auto => tui.current_id.with("auto").with(tui.current_node_index),
        }
    }
}

impl From<egui::Id> for TuiId {
    #[inline]
    fn from(value: egui::Id) -> Self {
        Self::Hiarchy(value)
    }
}
impl From<&str> for TuiId {
    #[inline]
    fn from(value: &str) -> Self {
        Self::Hiarchy(egui::Id::new(value))
    }
}

/// Helper function to generate TuiID that takes into account element hiarchy to avoid duplicated
/// ids
#[inline]
pub fn tid<T>(id: T) -> TuiId
where
    T: std::hash::Hash,
{
    TuiId::Hiarchy(egui::Id::new(id))
}

////////////////////////////////////////////////////////////////////////////////

/// Taffy layout state that stores calculated taffy node layout and hiarchy
struct TaffyState {
    taffy: TaffyTree<Context>,

    last_size: egui::Vec2,
    items: HashMap<egui::Id, NodeId>,
}

impl TaffyState {
    fn new() -> Self {
        Self {
            taffy: TaffyTree::new(),
            last_size: egui::Vec2::ZERO,
            items: HashMap::default(),
        }
    }

    fn layout(&self, node_id: NodeId) -> Layout {
        *self.taffy.layout(node_id).unwrap()
    }
}

////////////////////////////////////////////////////////////////////////////////

/// Helper structure to provide more egonomic API for child ui container creation
#[must_use]
pub struct TuiBuilder<'r, 'a>
where
    'a: 'r,
{
    tui: &'r mut Tui<'a>,
    id: TuiId,
    style: Option<taffy::Style>,
}

////////////////////////////////////////////////////////////////////////////////

/// Helper trait to reduce code boilerplate
pub trait AsTuiBuilder<'r, 'a>: Sized
where
    'a: 'r,
{
    /// Initialize creation of tui new child node
    fn tui(self) -> TuiBuilder<'r, 'a>;
}

impl<'r, 'a> AsTuiBuilder<'r, 'a> for &'r mut Tui<'a>
where
    'a: 'r,
{
    #[inline]
    fn tui(self) -> TuiBuilder<'r, 'a> {
        TuiBuilder {
            tui: self,
            style: None,
            id: TuiId::Auto,
        }
    }
}

impl<'r, 'a> AsTuiBuilder<'r, 'a> for TuiBuilder<'r, 'a>
where
    'a: 'r,
{
    #[inline]
    fn tui(self) -> TuiBuilder<'r, 'a> {
        self
    }
}

impl<'r, 'a, T> TuiBuilderLogic<'r, 'a> for T
where
    T: AsTuiBuilder<'r, 'a>,
    'a: 'r,
{
    // Use default implementation
}

////////////////////////////////////////////////////////////////////////////////

/// Trait that implements TuiBuilder logic for child node creation in Tui UI.
pub trait TuiBuilderLogic<'r, 'a>: AsTuiBuilder<'r, 'a> + Sized
where
    'a: 'r,
{
    /// Set child node id
    #[inline]
    fn id(self, id: impl Into<TuiId>) -> TuiBuilder<'r, 'a> {
        let mut tui = self.tui();
        tui.id = id.into();
        tui
    }

    /// Set child node style
    #[inline]
    fn style(self, style: taffy::Style) -> TuiBuilder<'r, 'a> {
        let mut tui = self.tui();
        tui.style = Some(style);
        tui
    }

    /// Set child node id and style
    #[inline]
    fn id_style(self, id: impl Into<TuiId>, style: taffy::Style) -> TuiBuilder<'r, 'a> {
        let mut tui = self.tui();
        tui.id = id.into();
        tui.style = Some(style);
        tui
    }

    /// Mutate child node style
    #[inline]
    fn mut_style(self, f: impl FnOnce(&mut taffy::Style)) -> TuiBuilder<'r, 'a> {
        let mut tui = self.tui();
        f(tui.style.get_or_insert_with(Default::default));
        tui
    }

    /// Add tui node as children to this node
    fn add<T>(self, f: impl FnOnce(&mut Tui<'_>) -> T) -> T {
        let tui = self.tui();
        tui.tui.add_children_inner(
            tui.id,
            tui.style.unwrap_or_default(),
            Option::<fn(&mut egui::Ui)>::None,
            f,
        )
    }

    /// Add tui node as children to this node and draw popup background
    fn add_with_background<T>(self, f: impl FnOnce(&mut Tui<'_>) -> T) -> T {
        self.add_with_background_ui(
            |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let available_space = ui.available_size();
                    let (id, rect) = ui.allocate_space(available_space);
                    let _response = ui.interact(rect, id, egui::Sense::click_and_drag());
                    // Background is not transparent to events
                });
            },
            f,
        )
    }

    /// Add tui node as children to this node and draw simple group Frame background
    fn add_with_border<T>(self, f: impl FnOnce(&mut Tui<'_>) -> T) -> T {
        self.add_with_background_ui(
            |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    let available_space = ui.available_size();
                    let (_id, _rect) = ui.allocate_space(available_space);
                    // Background is transparent to events
                });
            },
            f,
        )
    }

    /// Add tui node as children to this node and draw custom background
    ///
    /// See [`TuiBuilderLogic::add_with_background`] for example
    fn add_with_background_ui<T>(
        self,
        content: impl FnOnce(&mut egui::Ui),
        f: impl FnOnce(&mut Tui<'_>) -> T,
    ) -> T {
        let tui = self.tui();
        tui.tui
            .add_children_inner(tui.id, tui.style.unwrap_or_default(), Some(content), f)
    }

    /// Add scroll area as leaf node and draw background for it
    fn add_scroll_area_with_background<T>(self, content: impl FnOnce(&mut Ui) -> T) -> T {
        let mut tui = self.tui();
        tui = tui.mut_style(|style| {
            style.min_size = taffy::Size {
                width: Dimension::Length(0.),
                height: Dimension::Length(0.),
            };
        });

        tui.add_with_background(move |tui| {
            let s = LengthPercentageAuto::Length(
                0.3 * tui.ui.text_style_height(&egui::TextStyle::Body),
            );
            let style = taffy::Style {
                margin: Rect {
                    left: s,
                    right: s,
                    top: s,
                    bottom: s,
                },
                ..Default::default()
            };
            tui.tui().style(style).add_scroll_area(content)
        })
    }

    /// Add scroll area as leaf node
    fn add_scroll_area<T>(self, content: impl FnOnce(&mut Ui) -> T) -> T {
        let tui = self.tui();
        let limit = tui.tui.limit_scroll_area_size;
        tui.add_scroll_area_ext(limit, content)
    }

    /// Add scroll area as leaf node and provide custom limit for scroll area size
    fn add_scroll_area_ext<T>(self, limit: Option<f32>, content: impl FnOnce(&mut Ui) -> T) -> T {
        let tui = self.tui();
        tui.tui
            .add_scroll_area_ext(tui.id, tui.style.unwrap_or_default(), limit, content)
    }

    /// Add egui ui as tui leaf node
    #[inline]
    fn ui<T>(self, content: impl FnOnce(&mut Ui) -> T) -> T {
        self.ui_finite(content)
    }

    /// Add finite egui ui as tui leaf node
    fn ui_finite<T>(self, content: impl FnOnce(&mut Ui) -> T) -> T {
        self.ui_manual(|ui, _params| {
            let inner = content(ui);
            TuiContainerResponse {
                inner,
                min_size: ui.min_size(),
                intrinsic_size: None,
                max_size: ui.min_size(),
                infinite: egui::Vec2b::FALSE,
                scroll_area: false,
            }
        })
    }

    /// Add egui ui that can grow infinitely as tui leaf node
    fn ui_infinite<T>(self, content: impl FnOnce(&mut Ui) -> T) -> T {
        self.ui_manual(|ui, _params| {
            let inner = content(ui);
            TuiContainerResponse {
                inner,
                min_size: ui.min_size(),
                intrinsic_size: None,
                max_size: ui.min_size(),
                infinite: egui::Vec2b::TRUE,
                scroll_area: false,
            }
        })
    }

    /// Add egui ui as tui leaf node and provide custom information about necessary space for this
    /// node for layout calculation
    ///
    /// Useful when implementing [`TuiWidget`] trait
    fn ui_manual<T>(
        self,
        content: impl FnOnce(&mut Ui, TaffyContainerUi) -> TuiContainerResponse<T>,
    ) -> T {
        let tui = self.tui();
        tui.tui
            .add_container(tui.id, tui.style.unwrap_or_default(), content)
    }

    /// Add tui or egui widget that implements [`TuiWidget`]` as leaf node
    #[inline]
    fn ui_add<T: TuiWidget>(self, widget: T) -> T::Response {
        widget.taffy_ui(self.tui())
    }

    /// Add egui widget as leaf node and modify calculated used space information
    ///
    /// Useful when implementing [`TuiWidget`] trait
    fn ui_add_manual(
        self,
        f: impl FnOnce(&mut egui::Ui) -> Response,
        transform: impl FnOnce(
            TuiContainerResponse<Response>,
            &egui::Ui,
        ) -> TuiContainerResponse<Response>,
    ) -> Response {
        self.ui_manual(|ui, _params| {
            let response = f(ui);

            let resp = TuiContainerResponse {
                min_size: response.rect.size(),
                intrinsic_size: response.intrinsic_size,
                max_size: response.rect.size(),
                infinite: egui::Vec2b::FALSE,
                inner: response,
                scroll_area: false,
            };

            transform(resp, ui)
        })
    }

    /// Add egui label as child node
    #[inline]
    fn label(self, text: impl Into<egui::WidgetText>) -> Response {
        egui::Label::new(text).taffy_ui(self.tui())
    }

    /// Add egui heading as child node
    #[inline]
    fn heading(self, text: impl Into<egui::RichText>) -> Response {
        egui::Label::new(text.into().heading()).taffy_ui(self.tui())
    }

    /// Add egui separator  as child node
    #[inline]
    fn separator(self) -> Response {
        TaffySeparator::default().taffy_ui(self.tui())
    }
}

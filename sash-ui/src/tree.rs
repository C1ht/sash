// SPDX-License-Identifier: Apache-2.0

//! The widget tree: plain data with stable ids (owner, after M16: roles, labels, states
//! and the focus model from day one).
//!
//! Every node carries its semantics: the role, the accessible name and the states an
//! assistive technology would read (the AccessKit adapter maps onto this later). An
//! interactive widget without an accessible name is an [`Issue`]; [`crate::Ui`] checks
//! after every change (a panic in debug builds) and tests check every tree they build.
//!
//! Text is [`Text`]: a first-party string or sash-text's untrusted client text. Neither
//! is ever printed with its content by `Debug`, and sash-ui logs nothing.

use std::{borrow::Cow, fmt, rc::Rc};

use sash_text::ClientText;

use crate::{geometry::Rect, icon::IconSource, theme::TextLevel};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub(crate) u32);

/// Text shown or announced: a first-party string, or untrusted text from an app.
#[derive(Clone, PartialEq, Eq)]
pub enum Text {
    Own(Cow<'static, str>),
    Client(ClientText),
}

impl Text {
    pub fn is_empty(&self) -> bool {
        match self {
            Text::Own(s) => s.trim().is_empty(),
            Text::Client(c) => c.is_empty(),
        }
    }

    /// The text as sash-text takes it.
    pub(crate) fn shaped(&self) -> ClientText {
        match self {
            Text::Own(s) => ClientText::from(s.as_ref()),
            Text::Client(c) => c.clone(),
        }
    }
}

/// Never shows client text: an app's string stays out of every log by construction.
impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Text::Own(_) => f.write_str("Text::Own(..)"),
            Text::Client(_) => f.write_str("Text::Client(..)"),
        }
    }
}

impl From<&'static str> for Text {
    fn from(s: &'static str) -> Self {
        Text::Own(Cow::Borrowed(s))
    }
}

impl From<String> for Text {
    fn from(s: String) -> Self {
        Text::Own(Cow::Owned(s))
    }
}

impl From<ClientText> for Text {
    fn from(c: ClientText) -> Self {
        Text::Client(c)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// A node's size on one axis, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Fixed(i32),
    /// Its content's size.
    Content,
    /// A share of the space left on the parent's main axis (weight); the whole space
    /// on the cross axis.
    Flex(u16),
}

/// Where children sit on a container's cross axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Insets {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

impl Insets {
    pub const fn all(v: i32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Container {
    /// None: a stack (children on top of each other).
    pub axis: Option<Axis>,
    pub spacing: i32,
    pub padding: Insets,
    pub align: Align,
    /// One tab stop; arrows move inside it (roving focus).
    pub group: bool,
    pub name: Option<Text>,
}

#[derive(Clone, Debug)]
pub struct Label {
    pub text: Text,
    pub level: TextLevel,
}

#[derive(Clone)]
pub struct Icon {
    pub source: Rc<dyn IconSource>,
    /// Logical pixels (square).
    pub size: i32,
    /// None: decorative, hidden from assistive technologies.
    pub name: Option<Text>,
}

impl fmt::Debug for Icon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Icon")
            .field("size", &self.size)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    /// No fill until hovered or pressed.
    Flat,
    /// The derived accent fill, with text on it.
    Accent,
}

#[derive(Clone, Debug)]
pub struct Button {
    pub label: Option<Text>,
    pub icon: Option<Icon>,
    /// The accessible name when it is not the label (an icon-only button).
    pub name: Option<Text>,
    pub style: ButtonStyle,
}

#[derive(Clone, Debug)]
pub enum Kind {
    Container(Container),
    Label(Label),
    Icon(Icon),
    Button(Button),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Group,
    Label,
    Image,
    Button,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Activate,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: Kind,
    pub width: Size,
    pub height: Size,
    pub disabled: bool,
    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
    /// Where layout put it, logical pixels.
    pub(crate) rect: Rect,
}

/// Logical metrics of the widgets (docs/design.md; owner, M18c).
pub const BUTTON_HEIGHT: i32 = 34;
pub const BUTTON_PADDING: i32 = 13;
/// The padding where the spec sets 21 (the dock around its icons, the Recent plate).
pub const PLATE_PADDING: i32 = 21;
/// Icon sizes: in a title bar 15, in the search field and the footer 21, in the
/// tray 24.
pub const ICON_TITLE_BAR: i32 = 15;
pub const ICON_SEARCH: i32 = 21;
pub const ICON_FOOTER: i32 = 21;
pub const ICON_TRAY: i32 = 24;
/// An icon's size unless set ([`Node::icon_size`]): the footer's.
pub const ICON_SIZE: i32 = ICON_FOOTER;
pub const ICON_GAP: i32 = 8;

impl Node {
    fn new(kind: Kind, width: Size, height: Size) -> Self {
        Self {
            kind,
            width,
            height,
            disabled: false,
            parent: None,
            children: Vec::new(),
            rect: Rect::default(),
        }
    }

    fn container(axis: Option<Axis>) -> Self {
        Self::new(
            Kind::Container(Container {
                axis,
                spacing: 0,
                padding: Insets::default(),
                align: Align::Start,
                group: false,
                name: None,
            }),
            Size::Content,
            Size::Content,
        )
    }

    pub fn row() -> Self {
        Self::container(Some(Axis::Horizontal))
    }

    pub fn column() -> Self {
        Self::container(Some(Axis::Vertical))
    }

    pub fn stack() -> Self {
        Self::container(None)
    }

    pub fn label(text: impl Into<Text>) -> Self {
        Self::new(
            Kind::Label(Label {
                text: text.into(),
                level: TextLevel::Primary,
            }),
            Size::Content,
            Size::Content,
        )
    }

    /// A decorative icon (no accessible name; give one with [`Node::named`]).
    pub fn icon(source: Rc<dyn IconSource>) -> Self {
        Self::new(
            Kind::Icon(Icon {
                source,
                size: ICON_SIZE,
                name: None,
            }),
            Size::Content,
            Size::Content,
        )
    }

    /// A button showing `label`, which is also its accessible name.
    pub fn button(label: impl Into<Text>) -> Self {
        Self::new(
            Kind::Button(Button {
                label: Some(label.into()),
                icon: None,
                name: None,
                style: ButtonStyle::Flat,
            }),
            Size::Content,
            Size::Fixed(BUTTON_HEIGHT),
        )
    }

    /// A square button showing only an icon; `name` is what it is announced as.
    pub fn icon_button(source: Rc<dyn IconSource>, name: impl Into<Text>) -> Self {
        Self::new(
            Kind::Button(Button {
                label: None,
                icon: Some(Icon {
                    source,
                    size: ICON_SIZE,
                    name: None,
                }),
                name: Some(name.into()),
                style: ButtonStyle::Flat,
            }),
            Size::Fixed(BUTTON_HEIGHT),
            Size::Fixed(BUTTON_HEIGHT),
        )
    }

    pub fn width(mut self, size: Size) -> Self {
        self.width = size;
        self
    }

    pub fn height(mut self, size: Size) -> Self {
        self.height = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The accessible name (a container's group name, an icon's or a button's name).
    pub fn named(mut self, name: impl Into<Text>) -> Self {
        let name = Some(name.into());
        match &mut self.kind {
            Kind::Container(c) => c.name = name,
            Kind::Icon(i) => i.name = name,
            Kind::Button(b) => b.name = name,
            Kind::Label(_) => {}
        }
        self
    }

    pub fn level(mut self, level: TextLevel) -> Self {
        if let Kind::Label(l) = &mut self.kind {
            l.level = level;
        }
        self
    }

    /// An icon in front of a button's label.
    pub fn with_icon(mut self, source: Rc<dyn IconSource>) -> Self {
        if let Kind::Button(b) = &mut self.kind {
            b.icon = Some(Icon {
                source,
                size: ICON_SIZE,
                name: None,
            });
        }
        self
    }

    pub fn icon_size(mut self, size: i32) -> Self {
        match &mut self.kind {
            Kind::Icon(i) => i.size = size,
            Kind::Button(Button { icon: Some(i), .. }) => i.size = size,
            _ => {}
        }
        self
    }

    pub fn accent(mut self) -> Self {
        if let Kind::Button(b) = &mut self.kind {
            b.style = ButtonStyle::Accent;
        }
        self
    }

    pub fn spacing(mut self, spacing: i32) -> Self {
        if let Kind::Container(c) = &mut self.kind {
            c.spacing = spacing.max(0);
        }
        self
    }

    pub fn padding(mut self, padding: Insets) -> Self {
        if let Kind::Container(c) = &mut self.kind {
            c.padding = padding;
        }
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        if let Kind::Container(c) = &mut self.kind {
            c.align = align;
        }
        self
    }

    /// Makes a row or column one tab stop with arrow keys inside.
    pub fn group(mut self) -> Self {
        if let Kind::Container(c) = &mut self.kind {
            c.group = true;
        }
        self
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn children(&self) -> &[NodeId] {
        &self.children
    }

    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    /// Whether the widget takes input (exhaustive, so a new kind must decide).
    pub fn is_interactive(&self) -> bool {
        match self.kind {
            Kind::Button(_) => true,
            Kind::Container(_) | Kind::Label(_) | Kind::Icon(_) => false,
        }
    }

    pub fn role(&self) -> Option<Role> {
        match &self.kind {
            Kind::Container(c) => (c.group || c.name.is_some()).then_some(Role::Group),
            Kind::Label(_) => Some(Role::Label),
            Kind::Icon(i) => i.name.is_some().then_some(Role::Image),
            Kind::Button(_) => Some(Role::Button),
        }
    }

    /// The accessible name: the explicit one, else a label's or button's text.
    pub fn name(&self) -> Option<&Text> {
        let name = match &self.kind {
            Kind::Container(c) => c.name.as_ref(),
            Kind::Label(l) => Some(&l.text),
            Kind::Icon(i) => i.name.as_ref(),
            Kind::Button(b) => b.name.as_ref().or(b.label.as_ref()),
        };
        name.filter(|n| !n.is_empty())
    }

    pub fn actions(&self) -> &'static [Action] {
        if self.is_interactive() {
            &[Action::Activate]
        } else {
            &[]
        }
    }
}

/// A problem in the tree's semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issue {
    /// An interactive widget without an accessible name.
    NoName(NodeId),
}

/// One node as an assistive technology sees it.
#[derive(Clone, Debug)]
pub struct Semantics<'a> {
    pub id: NodeId,
    pub role: Role,
    pub name: Option<&'a Text>,
    pub rect: Rect,
    pub focusable: bool,
    pub focused: bool,
    pub disabled: bool,
    pub pressed: bool,
    pub actions: &'static [Action],
}

#[derive(Clone, Debug)]
pub struct Tree {
    nodes: Vec<Option<Node>>,
}

impl Tree {
    pub(crate) fn new(root: Node) -> Self {
        Self {
            nodes: vec![Some(root)],
        }
    }

    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.0 as usize)?.as_ref()
    }

    pub(crate) fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id.0 as usize)?.as_mut()
    }

    /// Adds `node` as the last child of `parent` (the root if `parent` is gone).
    pub(crate) fn add(&mut self, parent: NodeId, mut node: Node) -> NodeId {
        let parent = if self.get(parent).is_some() {
            parent
        } else {
            self.root()
        };
        let id = NodeId(self.nodes.len() as u32);
        node.parent = Some(parent);
        node.children.clear();
        self.nodes.push(Some(node));
        if let Some(p) = self.get_mut(parent) {
            p.children.push(id);
        }
        id
    }

    /// Removes `id` and its subtree (the root stays).
    pub(crate) fn remove(&mut self, id: NodeId) {
        if id == self.root() {
            return;
        }
        let Some(parent) = self.get(id).and_then(|n| n.parent) else {
            return;
        };
        if let Some(p) = self.get_mut(parent) {
            p.children.retain(|&c| c != id);
        }
        let mut stack = vec![id];
        while let Some(next) = stack.pop() {
            if let Some(node) = self.nodes.get_mut(next.0 as usize).and_then(Option::take) {
                stack.extend(node.children);
            }
        }
    }

    /// Every node, parents before children, children in order: the reading order.
    pub fn order(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![self.root()];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.get(id) {
                out.push(id);
                stack.extend(node.children.iter().rev());
            }
        }
        out
    }

    /// Whether `id` or an ancestor is disabled.
    pub fn is_disabled(&self, id: NodeId) -> bool {
        let mut next = Some(id);
        while let Some(n) = next.and_then(|i| self.get(i)) {
            if n.disabled {
                return true;
            }
            next = n.parent;
        }
        false
    }

    /// Interactive and enabled.
    pub fn is_focusable(&self, id: NodeId) -> bool {
        self.get(id).is_some_and(Node::is_interactive) && !self.is_disabled(id)
    }

    /// The nearest group container holding `id`.
    pub(crate) fn group_of(&self, id: NodeId) -> Option<NodeId> {
        let mut next = self.get(id)?.parent;
        while let Some(p) = next {
            let node = self.get(p)?;
            if matches!(&node.kind, Kind::Container(c) if c.group) {
                return Some(p);
            }
            next = node.parent;
        }
        None
    }

    pub fn issues(&self) -> Vec<Issue> {
        self.order()
            .into_iter()
            .filter(|&id| {
                self.get(id)
                    .is_some_and(|n| n.is_interactive() && n.name().is_none())
            })
            .map(Issue::NoName)
            .collect()
    }
}

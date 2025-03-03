pub mod layer;
pub mod path;
pub mod resource;

pub use path::{Format, Path};
pub use resource::{Edit, EditId, Snapshot, ViewResource};

use crate::cmd::Axis;
use crate::session::{Direction, Session, SessionCoords};
use crate::util;
use crate::view::layer::{FrameRange, Layer, LayerCoords, LayerId};

use rgx::kit::Rgba8;
use rgx::math::*;
use rgx::rect::Rect;

use nonempty::NonEmpty;

use miniserde::{Deserialize, Serialize};

use std::collections::btree_map;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io;
use std::ops::Deref;

/// View identifier.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Copy, Clone, Debug)]
pub struct ViewId(u16);

impl Default for ViewId {
    fn default() -> Self {
        ViewId(0)
    }
}

impl fmt::Display for ViewId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// View coordinates.
///
/// These coordinates are relative to the bottom left corner of the view.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ViewCoords<T>(Point2<T>);

impl<T> ViewCoords<T> {
    pub fn new(x: T, y: T) -> Self {
        Self(Point2::new(x, y))
    }
}

impl ViewCoords<i32> {
    pub fn clamp(&mut self, rect: Rect<i32>) {
        util::clamp(&mut self.0, rect);
    }
}

impl<T> Deref for ViewCoords<T> {
    type Target = Point2<T>;

    fn deref(&self) -> &Point2<T> {
        &self.0
    }
}

impl From<ViewCoords<f32>> for ViewCoords<i32> {
    fn from(other: ViewCoords<f32>) -> ViewCoords<i32> {
        ViewCoords::new(other.x.round() as i32, other.y.round() as i32)
    }
}

impl From<ViewCoords<i32>> for ViewCoords<f32> {
    fn from(other: ViewCoords<i32>) -> ViewCoords<f32> {
        ViewCoords::new(other.x as f32, other.y as f32)
    }
}

impl From<ViewCoords<f32>> for ViewCoords<u32> {
    fn from(other: ViewCoords<f32>) -> ViewCoords<u32> {
        ViewCoords::new(other.x.round() as u32, other.y.round() as u32)
    }
}

impl From<Point2<f32>> for ViewCoords<f32> {
    fn from(p: Point2<f32>) -> ViewCoords<f32> {
        ViewCoords::new(p.x, p.y)
    }
}

/// View extent information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewExtent {
    /// Frame width.
    pub fw: u32,
    /// Frame height.
    pub fh: u32,
    /// Number of frames.
    pub nframes: usize,
}

impl ViewExtent {
    pub fn new(fw: u32, fh: u32, nframes: usize) -> Self {
        ViewExtent { fw, fh, nframes }
    }

    /// Extent total width.
    pub fn width(&self) -> u32 {
        self.fw * self.nframes as u32
    }

    /// Extent total height.
    pub fn height(&self) -> u32 {
        self.fh
    }

    /// Rect containing the whole extent.
    pub fn rect(&self) -> Rect<u32> {
        Rect::origin(self.width(), self.height())
    }

    /// Rect containing a single frame.
    pub fn frame(&self, n: usize) -> Rect<u32> {
        let n = n as u32;
        Rect::new(self.fw * n, 0, self.fw * n + self.fw, self.fh)
    }

    /// Compute the frame index, given a point.
    /// Warning: can underflow.
    pub fn to_frame(self, p: ViewCoords<u32>) -> usize {
        (p.x / (self.fw as u32)) as usize
    }
}

/// Current state of the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewState {
    /// The view is okay. It doesn't need to be redrawn or saved.
    Okay,
    /// The view has been touched, the changes need to be stored in a snapshot.
    /// If the parameter is `Some`, the view extents were changed.
    Dirty(Option<ViewExtent>),
    /// The view is damaged, it needs to be redrawn from a snapshot.
    /// This happens when undo/redo is used.
    Damaged(Option<ViewExtent>),
    /// A layer has been touched.
    LayerDirty(LayerId),
    /// A layer is damaged, it needs to be redrawn from a snapshot.
    /// This happens when undo/redo is used.
    LayerDamaged(LayerId),
}

/// A view operation to be carried out by the renderer.
#[derive(Debug, Clone)]
pub enum ViewOp {
    /// Copy an area of the view to another area.
    Blit(Rect<f32>, Rect<f32>),
    /// Clear to a color.
    Clear(Rgba8),
    /// Yank the given area into the paste buffer.
    Yank(LayerId, Rect<i32>),
    /// Flips a given area horizontally or vertically.
    Flip(LayerId, Rect<i32>, Axis),
    /// Blit the paste buffer into the given area.
    Paste(Rect<i32>),
    /// Resize the view.
    Resize(u32, u32),
    /// Paint a single pixel.
    SetPixel(LayerId, Rgba8, i32, i32),
    /// Add a layer.
    AddLayer(LayerId, FrameRange),
    /// Remove a layer.
    RemoveLayer(LayerId),
}

/// A view on a sprite or image.
#[derive(Debug)]
pub struct View<R> {
    /// Frame width.
    pub fw: u32,
    /// Frame height.
    pub fh: u32,
    /// View offset relative to the session workspace.
    pub offset: Vector2<f32>,
    /// Identifier.
    pub id: ViewId,
    /// Zoom level.
    pub zoom: f32,
    /// List of operations to carry out on the view.  Cleared every frame.
    pub ops: Vec<ViewOp>,
    /// Whether the view is flipped in the X axis.
    pub flip_x: bool,
    /// Whether the view is flipped in the Y axis.
    pub flip_y: bool,
    /// Status of the file displayed by this view.
    pub file_status: FileStatus,
    /// State of the view.
    pub state: ViewState,
    /// Animation state of the sprite displayed by this view.
    pub animation: Animation<Rect<f32>>,
    /// View layers.
    pub layers: NonEmpty<Layer>,
    /// Currently active layer.
    pub active_layer_id: LayerId,
    /// View resource.
    pub resource: R,

    /// Which view snapshot has been saved to disk, if any.
    saved_snapshot: Option<EditId>,
}

/// View animation.
#[derive(Debug)]
pub struct Animation<T> {
    pub index: usize,
    pub frames: Vec<T>,
}

impl<T> Animation<T> {
    pub fn new(frames: Vec<T>) -> Self {
        Self { index: 0, frames }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn step(&mut self) {
        self.index = (self.index + 1) % self.len();
    }

    pub fn val(&self) -> &T {
        &self.frames[self.index]
    }
}

impl<R> std::ops::Deref for View<R> {
    type Target = R;

    fn deref(&self) -> &Self::Target {
        &self.resource
    }
}

impl<R> std::ops::DerefMut for View<R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.resource
    }
}

impl<R> View<R> {
    /// Create a new view. Takes a frame width and height.
    pub fn new(id: ViewId, fs: FileStatus, fw: u32, fh: u32, nframes: usize, resource: R) -> Self {
        let saved_snapshot = if let FileStatus::Saved(_) = &fs {
            Some(Default::default())
        } else {
            None
        };

        let origin = Rect::origin(fw as f32, fh as f32);
        let frames: Vec<_> = (0..nframes)
            .map(|i| origin + Vector2::new(i as f32 * fw as f32, 0.))
            .collect();

        Self {
            id,
            fw,
            fh,
            offset: Vector2::zero(),
            zoom: 1.,
            ops: Vec::new(),
            flip_x: false,
            flip_y: false,
            file_status: fs,
            animation: Animation::new(frames),
            state: ViewState::Okay,
            layers: NonEmpty::new(Layer::default()),
            active_layer_id: Default::default(),
            saved_snapshot,
            resource,
        }
    }

    /// View width. Basically frame-width times number of frames.
    pub fn width(&self) -> u32 {
        self.fw * self.animation.len() as u32
    }

    /// View height.
    pub fn height(&self) -> u32 {
        self.fh * self.layers.len() as u32
    }

    /// View width and height.
    pub fn size(&self) -> (u32, u32) {
        (self.width(), self.height())
    }

    /// View layer width and height.
    pub fn layer_size(&self) -> (u32, u32) {
        (self.width(), self.fh)
    }

    /// View file name, if any.
    pub fn file_storage(&self) -> Option<&FileStorage> {
        match self.file_status {
            FileStatus::New(ref f) => Some(f),
            FileStatus::Modified(ref f) => Some(f),
            FileStatus::Saved(ref f) => Some(f),
            FileStatus::NoFile => None,
        }
    }

    /// Extend the view by one frame.
    pub fn extend(&mut self) {
        let w = self.width() as f32;
        let fw = self.fw as f32;
        let fh = self.fh as f32;

        self.animation.frames.push(Rect::new(w, 0., w + fw, fh));

        self.resized();
    }

    /// Shrink the view by one frame.
    pub fn shrink(&mut self) {
        // Don't allow the view to have zero frames.
        if self.animation.len() > 1 {
            self.animation.frames.pop();
            self.resized();
        }
    }

    /// Extend the view by one frame, by cloning an existing frame,
    /// by index.
    pub fn extend_clone(&mut self, index: i32) {
        let width = self.width() as f32;
        let (fw, fh) = (self.fw as f32, self.fh as f32);

        let index = if index == -1 {
            self.animation.len() - 1
        } else {
            index as usize
        };

        self.extend();
        self.ops.push(ViewOp::Blit(
            Rect::new(fw * index as f32, 0., fw * (index + 1) as f32, fh),
            Rect::new(width, 0., width + fw, fh),
        ));
    }

    /// Resize view frames to the given size.
    pub fn resize_frames(&mut self, fw: u32, fh: u32) {
        self.reset(ViewExtent::new(fw, fh, self.animation.len()));
        self.resized();
    }

    /// Activate a layer.
    pub fn activate_layer(&mut self, l: LayerId) -> bool {
        if self.layers.len() > l {
            self.active_layer_id = l;
            return true;
        }
        false
    }

    /// Activate the next layer.
    pub fn activate_next_layer(&mut self) -> bool {
        if self.layers.len() > self.active_layer_id + 1 {
            self.active_layer_id += 1;
            return true;
        }
        false
    }

    /// Activate the previous layer.
    pub fn activate_prev_layer(&mut self) -> bool {
        if self.active_layer_id > 0 {
            self.active_layer_id -= 1;
            return true;
        }
        false
    }

    /// Get the active layer.
    pub fn active_layer(&self) -> &Layer {
        let index: usize = self.active_layer_id;

        self.layers
            .get(index)
            .expect("there is always an active layer")
    }

    /// Push an empty layer.
    pub fn push_layer(&mut self) -> LayerId {
        let top = self
            .layers
            .iter()
            .max_by_key(|l| l.index)
            .expect("there is always at least one layer")
            .index;
        let range = FrameRange::Full;
        let id = self.layers.len();

        self.layers.push(Layer::new(range.clone(), top + 1));
        self.ops.push(ViewOp::AddLayer(id, range));

        id
    }

    /// Remove a layer.
    pub fn remove_layer(&mut self, id: LayerId) {
        // TODO: For now, we only allow the last layer to be removed.
        debug_assert_eq!(id, self.layers.len() - 1);

        self.layers.pop();
        self.ops.push(ViewOp::RemoveLayer(id));
        self.active_layer_id = self.layers.len() - 1;
    }

    /// Clear the view to a color.
    pub fn clear(&mut self, color: Rgba8) {
        self.ops.push(ViewOp::Clear(color));
        self.touch();
    }

    pub fn paint_color(&mut self, color: Rgba8, x: i32, y: i32) {
        self.ops
            .push(ViewOp::SetPixel(self.active_layer_id, color, x, y));
    }

    pub fn yank(&mut self, area: Rect<i32>) {
        self.ops.push(ViewOp::Yank(self.active_layer_id, area));
    }

    pub fn flip(&mut self, area: Rect<i32>, dir: Axis) {
        self.ops.push(ViewOp::Flip(self.active_layer_id, area, dir));
    }

    pub fn paste(&mut self, area: Rect<i32>) {
        self.ops.push(ViewOp::Paste(area));
        self.touch_layer();
    }

    /// Slice the view into the given number of frames.
    pub fn slice(&mut self, nframes: usize) -> bool {
        if nframes > 0 && self.width() % nframes as u32 == 0 {
            let fw = self.width() / nframes as u32;
            self.reset(ViewExtent::new(fw, self.fh, nframes));
            // FIXME: This is very inefficient. Since the actual frame contents
            // haven't changed, we don't need to create a full snapshot. We just
            // have to record how many frames are in this snapshot.
            self.touch();

            return true;
        }
        false
    }

    /// Restore a view layer to a given snapshot.
    pub fn restore_layer(&mut self, eid: EditId, layer: LayerId) {
        self.layer_damaged(layer);
        self.refresh_file_status(eid);
    }

    /// Restore a view to a given snapshot and extent.
    pub fn restore_extent(&mut self, eid: EditId, extent: ViewExtent) {
        self.damaged(Some(extent));
        self.reset(extent);
        self.refresh_file_status(eid);
    }

    /// Restore a view to a given snapshot.
    pub fn restore(&mut self, eid: EditId) {
        self.damaged(None);
        self.refresh_file_status(eid);
    }

    /// If the snapshot was saved to disk, we mark the view as saved too.
    /// Otherwise, if the view was saved before restoring the snapshot,
    /// we mark it as modified.
    pub fn refresh_file_status(&mut self, eid: EditId) {
        match self.file_status {
            FileStatus::Modified(ref f) if self.is_snapshot_saved(eid) => {
                self.file_status = FileStatus::Saved(f.clone());
            }
            FileStatus::Saved(ref f) => {
                self.file_status = FileStatus::Modified(f.clone());
            }
            _ => {
                // TODO
            }
        }
    }

    /// Set the view state to `Okay`.
    pub fn okay(&mut self) {
        self.state = ViewState::Okay;
    }

    /// Return the view area, including the offset.
    pub fn rect(&self) -> Rect<f32> {
        Rect::new(
            self.offset.x,
            self.offset.y,
            self.offset.x + self.width() as f32 * self.zoom,
            self.offset.y + self.height() as f32 * self.zoom,
        )
    }

    /// Return the area of the given layer, including the view offset.
    pub fn layer_rect(&self, index: usize) -> Rect<f32> {
        Rect::new(
            self.offset.x,
            self.offset.y + (self.fh * index as u32) as f32 * self.zoom,
            self.offset.x + self.width() as f32 * self.zoom,
            self.offset.y + (self.fh * (index + 1) as u32) as f32 * self.zoom,
        )
    }

    /// The offset of the layer at the given index.
    pub fn layer_offset(&self, index: usize, zoom: f32) -> Vector2<f32> {
        Vector2::new(0., self.fh as f32 * index as f32 * zoom)
    }

    /// Check whether the session coordinates are contained within the view.
    pub fn contains(&self, p: SessionCoords) -> Option<LayerId> {
        if self.rect().contains(*p) {
            for (i, _) in self.layers.iter().enumerate() {
                if self.layer_rect(i).contains(*p) {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Get the center of the view.
    pub fn center(&self) -> ViewCoords<f32> {
        ViewCoords::new(self.width() as f32 / 2., self.height() as f32 / 2.)
    }

    /// Get the center of the active layer.
    pub fn active_layer_center(&self) -> ViewCoords<f32> {
        ViewCoords::new(
            self.width() as f32 / 2.,
            self.layer_offset(self.active_layer_id, 1.).y + self.fh as f32 / 2.,
        )
    }

    /// Layer has been modified. Called when using the brush on the view,
    /// or resizing the view.
    pub fn touch_layer(&mut self) {
        if let FileStatus::Saved(ref f) = self.file_status {
            self.file_status = FileStatus::Modified(f.clone());
        }
        if self.state == ViewState::Okay {
            self.state = ViewState::LayerDirty(self.active_layer_id);
        }
    }

    /// View has been modified. Called when using the brush on the view,
    /// or resizing the view.
    pub fn touch(&mut self) {
        if let FileStatus::Saved(ref f) = self.file_status {
            self.file_status = FileStatus::Modified(f.clone());
        }
        if self.state == ViewState::Okay {
            self.state = ViewState::Dirty(None);
        }
    }

    /// View should be considered damaged and needs to be restored from snapshot.
    /// Used when undoing or redoing changes.
    pub fn damaged(&mut self, extent: Option<ViewExtent>) {
        self.state = ViewState::Damaged(extent);
    }

    /// Layer should be considered damaged and needs to be restored from snapshot.
    /// Used when undoing or redoing changes.
    pub fn layer_damaged(&mut self, layer: LayerId) {
        self.state = ViewState::LayerDamaged(layer);
    }

    /// Check whether the view is damaged.
    pub fn is_damaged(&self) -> bool {
        matches!(
            self.state,
            ViewState::Damaged(_) | ViewState::LayerDamaged(_)
        )
    }

    /// Check whether the view is dirty.
    pub fn is_dirty(&self) -> bool {
        matches!(self.state, ViewState::Dirty(_) | ViewState::LayerDirty(_))
    }

    /// Check whether the view is resized.
    pub fn is_resized(&self) -> bool {
        matches!(self.state, ViewState::Dirty(Some(_)))
    }

    /// Check whether the view is okay.
    pub fn is_okay(&self) -> bool {
        self.state == ViewState::Okay
    }

    /// Return the file status as a string.
    pub fn status(&self) -> String {
        self.file_status.to_string()
    }

    /// Return the view extent.
    pub fn extent(&self) -> ViewExtent {
        ViewExtent::new(self.fw, self.fh, self.animation.len())
    }

    /// Return the view bounds, as an origin-anchored rectangle.
    pub fn bounds(&self) -> Rect<i32> {
        Rect::origin(self.width() as i32, self.height() as i32)
    }

    /// Return the view layer bounds, as an origin-anchored rectangle.
    pub fn layer_bounds(&self) -> Rect<i32> {
        Rect::origin(self.width() as i32, self.fh as i32)
    }

    ////////////////////////////////////////////////////////////////////////////

    fn resized(&mut self) {
        if let FileStatus::Saved(ref f) = self.file_status {
            self.file_status = FileStatus::Modified(f.clone());
        }
        if self.state == ViewState::Okay {
            self.state = ViewState::Dirty(Some(self.extent()));
        }
        self.ops.push(ViewOp::Resize(self.width(), self.fh));
    }

    /// Check whether the given snapshot has been saved to disk.
    fn is_snapshot_saved(&self, id: EditId) -> bool {
        self.saved_snapshot == Some(id)
    }

    /// Mark the view as saved at a given snapshot.
    fn saved(&mut self, id: EditId, storage: FileStorage) {
        self.file_status = FileStatus::Saved(storage);
        self.saved_snapshot = Some(id);
    }

    /// Reset the view by providing frame size and number of frames.
    fn reset(&mut self, extent: ViewExtent) {
        self.fw = extent.fw;
        self.fh = extent.fh;

        let mut frames = Vec::new();
        let origin = Rect::origin(self.fw as f32, self.fh as f32);

        for i in 0..extent.nframes {
            frames.push(origin + Vector2::new(i as f32 * self.fw as f32, 0.));
        }
        self.animation = Animation::new(frames);
    }
}

impl View<ViewResource> {
    /// Add a new layer with optional pixels.
    pub fn add_layer(&mut self, pixels: Option<Vec<Rgba8>>) -> LayerId {
        let id = self.push_layer();

        self.resource.add_layer(
            id,
            self.extent(),
            pixels.unwrap_or(vec![
                Rgba8::TRANSPARENT;
                self.width() as usize * self.fh as usize
            ]),
        );

        id
    }

    /// Get the color at the given view coordinate.
    pub fn color_at(&self, l: LayerId, p: LayerCoords<u32>) -> Option<&Rgba8> {
        self.resource
            .current_snapshot(l)
            .and_then(|(snapshot, pixels)| {
                snapshot
                    .layer_coord_to_index(p)
                    .and_then(|idx| pixels.get(idx))
            })
    }

    /// Restore a view snapshot (undo/redo an edit).
    pub fn restore_snapshot(&mut self, dir: Direction) {
        let result = if dir == Direction::Backward {
            self.resource.history_prev()
        } else {
            self.resource.history_next()
        };

        match result {
            Some((eid, Edit::LayerPainted(layer))) => {
                self.restore_layer(eid, layer);
            }
            Some((eid, Edit::LayerAdded(layer))) => {
                match dir {
                    Direction::Backward => {
                        self.remove_layer(layer);
                    }
                    Direction::Forward => {
                        // TODO: This relies on the fact that `remove_layer` can
                        // only remove the last layer.
                        let layer_id = self.push_layer();
                        debug_assert!(layer_id == layer);
                    }
                };
                self.refresh_file_status(eid);
            }
            Some((eid, Edit::ViewResized(_, from, to))) => {
                let extent = match dir {
                    Direction::Backward => from,
                    Direction::Forward => to,
                };
                self.restore_extent(eid, extent);
            }
            Some((eid, Edit::ViewPainted(_))) => {
                self.restore(eid);
            }
            Some((_, Edit::Initial)) => {}
            None => {}
        }
    }

    pub fn save_as(&mut self, storage: &FileStorage) -> io::Result<usize> {
        let active_layer_id = self.active_layer_id;
        let ext = self.extent();
        let nlayers = self.layers.len();

        let (edit_id, written) = match &storage {
            FileStorage::Single(path) => {
                {
                    let mut path_copy = path.clone();
                    path_copy.pop();
                    std::fs::create_dir_all(path_copy.as_path())?;
                }

                if nlayers > 1 {
                    let written = self.resource.save_archive(path)?;
                    let edit_id = self.resource.cursor;

                    (edit_id, written)
                } else {
                    let edit_id = self.save_layer_rect_as(active_layer_id, ext.rect(), &path)?;

                    (edit_id, (ext.width() * ext.height()) as usize)
                }
            }
            FileStorage::Range(paths) if nlayers == 1 => {
                for (i, path) in paths.iter().enumerate() {
                    self.save_layer_rect_as(active_layer_id, ext.frame(i), path)?;
                }

                let edit_id = self.resource.current_edit();

                (edit_id, paths.len() * (ext.fw * ext.fh) as usize)
            }
            FileStorage::Range(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "range storage is not supported for more than one layer",
                ));
            }
        };

        // Mark the view as saved at a specific snapshot and with the given path.
        match self.file_status {
            FileStatus::Modified(ref curr_fs) | FileStatus::New(ref curr_fs) => {
                if curr_fs == storage {
                    self.saved(edit_id, storage.clone());
                }
            }
            FileStatus::NoFile => {
                self.saved(edit_id, storage.clone());
            }
            FileStatus::Saved(_) => {}
        }

        Ok(written)
    }

    /// Save part of a layer to disk.
    fn save_layer_rect_as(
        &mut self,
        layer_id: LayerId,
        rect: Rect<u32>,
        path: &std::path::Path,
    ) -> io::Result<EditId> {
        // Only allow overwriting of files if it's the file of the view being saved.
        if path.exists() && self.file_storage().map_or(true, |f| !f.contains(path)) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("\"{}\" already exists", path.display()),
            ));
        }
        let (e_id, _) = self.save_layer(layer_id, rect, &path)?;

        Ok(e_id)
    }
}

///////////////////////////////////////////////////////////////////////////////

/// Status of the underlying file displayed by the view.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum FileStatus {
    /// There is no file being displayed.
    NoFile,
    /// The file is new and unsaved.
    New(FileStorage),
    /// The file is saved and unmodified.
    Saved(FileStorage),
    /// The file has been modified since the last save.
    Modified(FileStorage),
}

impl ToString for FileStatus {
    fn to_string(&self) -> String {
        match self {
            FileStatus::NoFile => String::new(),
            FileStatus::Saved(ref storage) => format!("{}", storage),
            FileStatus::New(ref storage) => format!("{} [new]", storage),
            FileStatus::Modified(ref storage) => format!("{} [modified]", storage),
        }
    }
}

/// Representation of the view data on disk.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum FileStorage {
    /// Stored as a range of files.
    Range(NonEmpty<std::path::PathBuf>),
    /// Stored as a single file.
    Single(std::path::PathBuf),
}

impl fmt::Display for FileStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Range(paths) => {
                let parent = paths.first().parent();

                if paths.iter().all(|p| p.parent() == parent) {
                    let first = paths
                        .first()
                        .file_stem()
                        .expect("the path has a file stem")
                        .to_string_lossy()
                        .into_owned();
                    let last = paths
                        .last()
                        .file_name()
                        .expect("the path has a file name")
                        .to_string_lossy();

                    let first = if let Some(parent) = parent {
                        parent.join(first)
                    } else {
                        first.into()
                    };

                    write!(f, "{} .. {}", first.display(), last)
                } else {
                    write!(f, "*")
                }
            }
            Self::Single(path) => write!(f, "{}", path.display()),
        }
    }
}

impl From<&std::path::Path> for FileStorage {
    fn from(p: &std::path::Path) -> Self {
        FileStorage::Single(p.into())
    }
}

impl From<std::path::PathBuf> for FileStorage {
    fn from(p: std::path::PathBuf) -> Self {
        FileStorage::Single(p)
    }
}

impl FileStorage {
    pub fn contains<P: AsRef<std::path::Path>>(&self, p: P) -> bool {
        match self {
            Self::Single(buf) => buf.as_path() == p.as_ref(),
            Self::Range(bufs) => bufs.iter().any(|buf| buf.as_path() == p.as_ref()),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////

/// Manages views.
#[derive(Debug)]
pub struct ViewManager<R> {
    /// Currently active view.
    pub active_id: ViewId,

    /// View dictionary.
    views: BTreeMap<ViewId, View<R>>,

    /// The next `ViewId`.
    next_id: ViewId,

    /// A last-recently-used list of views.
    lru: VecDeque<ViewId>,
}

impl<R> ViewManager<R> {
    /// Maximum number of views in the view LRU list.
    const MAX_LRU: usize = Session::MAX_VIEWS;

    /// New empty view manager.
    pub fn new() -> Self {
        Self {
            active_id: ViewId::default(),
            next_id: ViewId(1),
            views: BTreeMap::new(),
            lru: VecDeque::new(),
        }
    }

    /// Add a view.
    pub fn add(&mut self, fs: FileStatus, w: u32, h: u32, nframes: usize, resource: R) -> ViewId {
        let id = self.gen_id();
        let view = View::new(id, fs, w, h, nframes, resource);

        self.views.insert(id, view);

        id
    }

    /// Remove a view.
    pub fn remove(&mut self, id: ViewId) {
        self.views.remove(&id);
        self.lru.retain(|v| *v != id);

        self.active_id = self
            .recent()
            .or_else(|| self.last().map(|v| v.id))
            .unwrap_or(ViewId::default());
    }

    /// Return the id of the last recently active view, if any.
    pub fn recent(&self) -> Option<ViewId> {
        self.lru.front().cloned()
    }

    /// Return the currently active view, if any.
    pub fn active(&self) -> Option<&View<R>> {
        self.views.get(&self.active_id)
    }

    /// Return the currently active view mutably, if any.
    pub fn active_mut(&mut self) -> Option<&mut View<R>> {
        self.views.get_mut(&self.active_id)
    }

    /// Activate a view.
    pub fn activate(&mut self, id: ViewId) {
        debug_assert!(
            self.views.contains_key(&id),
            "the view being activated exists"
        );
        if self.active_id == id {
            return;
        }
        self.active_id = id;
        self.lru.push_front(id);
        self.lru.truncate(Self::MAX_LRU);
    }

    /// Iterate over views.
    pub fn iter(&self) -> btree_map::Values<'_, ViewId, View<R>> {
        self.views.values()
    }

    /// Iterate over views, mutably.
    pub fn iter_mut(&mut self) -> btree_map::ValuesMut<'_, ViewId, View<R>> {
        self.views.values_mut()
    }

    /// Get a view, mutably.
    pub fn get(&self, id: ViewId) -> Option<&View<R>> {
        self.views.get(&id)
    }

    /// Get a view, mutably.
    pub fn get_mut(&mut self, id: ViewId) -> Option<&mut View<R>> {
        self.views.get_mut(&id)
    }

    /// Find a view.
    pub fn find<F>(&self, f: F) -> Option<&View<R>>
    where
        for<'r> F: Fn(&'r &View<R>) -> bool,
    {
        self.iter().find(f)
    }

    /// Iterate over view ids.
    pub fn ids(&self) -> impl DoubleEndedIterator<Item = ViewId> + '_ {
        self.views.keys().cloned()
    }

    /// Get `ViewId` *after* given id.
    pub fn after(&self, id: ViewId) -> Option<ViewId> {
        self.range(id..).nth(1)
    }

    /// Get `ViewId` *before* given id.
    pub fn before(&self, id: ViewId) -> Option<ViewId> {
        self.range(..id).next_back()
    }

    /// Get the first view.
    pub fn first(&self) -> Option<&View<R>> {
        self.iter().next()
    }

    /// Get the first view, mutably.
    pub fn first_mut(&mut self) -> Option<&mut View<R>> {
        self.iter_mut().next()
    }

    /// Get the last view.
    pub fn last(&self) -> Option<&View<R>> {
        self.iter().next_back()
    }

    /// Get view id range.
    pub fn range<G>(&self, r: G) -> impl DoubleEndedIterator<Item = ViewId> + '_
    where
        G: std::ops::RangeBounds<ViewId>,
    {
        self.views.range(r).map(|(id, _)| *id)
    }

    /// Whether there are views.
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    /// Generate a new view id.
    fn gen_id(&mut self) -> ViewId {
        let ViewId(id) = self.next_id;
        self.next_id = ViewId(id + 1);

        ViewId(id)
    }
}

impl ViewManager<ViewResource> {
    pub fn get_snapshot_safe(
        &self,
        id: ViewId,
        layer_id: LayerId,
    ) -> Option<(&Snapshot, &[Rgba8])> {
        self.views
            .get(&id)
            .and_then(|v| v.resource.current_snapshot(layer_id))
    }

    pub fn get_snapshot(&self, id: ViewId, layer_id: LayerId) -> (&Snapshot, &[Rgba8]) {
        self.get_snapshot_safe(id, layer_id).expect(&format!(
            "layer #{} of view #{} must exist and have an associated snapshot",
            layer_id, id
        ))
    }

    pub fn get_snapshot_rect(
        &self,
        id: ViewId,
        layer_id: LayerId,
        rect: &Rect<i32>,
    ) -> Option<(&Snapshot, Vec<Rgba8>)> {
        self.views
            .get(&id)
            .map(|v| &v.resource)
            .and_then(|v| v.layers.get(&layer_id))
            .expect(&format!(
                "view #{} with layer #{} must exist and have an associated snapshot",
                id, layer_id
            ))
            .get_snapshot_rect(rect)
    }
}

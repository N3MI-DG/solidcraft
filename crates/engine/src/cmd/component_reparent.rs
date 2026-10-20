//! Moving a placement into another component, or out to the design, where it stays in the world.
//! Its local transform is worked out again in the new parent's frame. A parent placed more than
//! once is one definition: what goes in it goes in every instance, placed by the one instance
//! named (`parent_path`, else the active placement) and in the same place in the others. Paths
//! through the placement change: the session's paths (selection, active and hidden placements,
//! hidden folders) follow it, and a move that would leave a feature failing that worked before
//! (a face attachment found through the old path) is refused.

use super::*;
use crate::Sel;

/// The `parent_path` parameter: the placement of the parent to place by (`[]`: the design).
pub(super) fn parent_path(p: &Value, cmd: &str) -> Result<Option<Vec<u64>>> {
    let Some(value) = p.get("parent_path") else { return Ok(None) };
    value
        .as_array()
        .filter(|a| a.len() <= 64)
        .and_then(|a| a.iter().map(Value::as_u64).collect::<Option<Vec<_>>>())
        .map(Some)
        .ok_or_else(|| bad(cmd, "`parent_path` must list 0..64 occurrence ids"))
}

/// The transform a copy of the placement at `path` takes in `parent` to sit where the placement
/// is in the world, then moved by `translate` (in the world). The parent is placed by
/// [`placement_of`]: in its other instances the copy sits in the same place within them.
pub(super) fn in_place(s: &Session, path: &[u64], parent: u64, parent_path: Option<&[u64]>, translate: Vec3, cmd: &str) -> Result<Mat> {
    let doc = s.placed_document();
    let component =
        path.last().and_then(|id| doc.occurrences.iter().find(|o| o.id == *id)).map(|o| o.component).ok_or_else(|| bad(cmd, "no placement"))?;
    let world = doc.path_transform(component, path).ok_or_else(|| bad(cmd, "that placement is gone"))?;
    let (_, frame) = placement_of(s, &doc, parent, parent_path, cmd)?;
    let inverse = solidcraft_doc::mat_inverse(&frame).ok_or_else(|| bad(cmd, "the parent's frame is not invertible"))?;
    Ok(solidcraft_doc::mat_mul(&inverse, &solidcraft_doc::mat_mul(&translation(translate), &world)))
}

/// The placement of `component` a move or paste is placed by (the design is the empty path): its
/// path and world frame. `path` when given (it must lead to `component`), else its only
/// placement, else the active placement when that is one of it.
pub(super) fn placement_of(s: &Session, doc: &solidcraft_doc::Document, component: u64, path: Option<&[u64]>, cmd: &str) -> Result<(Vec<u64>, Mat)> {
    let name = || doc.components.iter().find(|c| c.id == component).map(|c| c.name.clone()).unwrap_or_else(|| "that component".into());
    if let Some(path) = path {
        let frame = doc.path_transform(component, path).ok_or_else(|| bad(cmd, format!("that path is not a placement of {}", name())))?;
        return Ok((path.to_vec(), frame));
    }
    let all: Vec<(u64, Vec<u64>, Mat)> = doc.placements().into_iter().filter(|(c, _, _)| *c == component).collect();
    match all.as_slice() {
        [(_, path, frame)] => Ok((path.clone(), *frame)),
        [] => Err(bad(cmd, "that component is not placed in the design")),
        _ => all
            .iter()
            .find(|(_, path, _)| s.active_occurrence_path.as_deref() == Some(path.as_slice()))
            .map(|(_, path, frame)| (path.clone(), *frame))
            .ok_or_else(|| bad(cmd, format!("{} is placed more than once: activate the placement to work in, or give it as `parent_path`", name()))),
    }
}

/// `path` with its leading `from` replaced by `to`, when it starts with `from`.
fn rebased(path: &[u64], from: &[u64], to: &[u64]) -> Option<Vec<u64>> {
    path.strip_prefix(from).map(|rest| to.iter().chain(rest).copied().collect())
}

pub(super) fn reparent(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "occurrence.reparent";
    let id = occurrence(s, p, cmd)?;
    let to = component(s, p, "parent", cmd)?;
    let o = s.doc.occurrences.iter().find(|o| o.id == id).cloned().ok_or_else(|| bad(cmd, "no occurrence"))?;
    if o.parent == to {
        return Ok(json!({"occurrence": id, "parent": to, "path": occurrence_path(s, p, id, cmd)?}));
    }
    if s.doc.inside_component(to, o.component) {
        return Err(bad(cmd, format!("{} can't go inside itself or a component within it", o.name)));
    }
    if s.doc.marker.is_some() {
        return Err(bad(cmd, "finish timeline rollback before moving placements"));
    }
    if s.held_by_rigid_group(id).is_some() {
        return Err(bad(cmd, format!("{} is held by a rigid group: Unlock Contents (component.unlock) first", o.name)));
    }
    // From the placement named (`path`), into the parent's placement named (`parent_path`).
    let moved = occurrence_path(s, p, id, cmd)?;
    let to_given = parent_path(p, cmd)?;
    let doc = s.placed_document();
    let (from_path, from_frame) = placement_of(s, &doc, o.parent, moved.split_last().map(|(_, up)| up), cmd)?;
    let (to_path, to_frame) = placement_of(s, &doc, to, to_given.as_deref(), cmd)?;
    let local = doc.occurrences.iter().find(|x| x.id == id).map(|x| x.transform).unwrap_or(o.transform);
    let world = solidcraft_doc::mat_mul(&from_frame, &local);
    let inverse = solidcraft_doc::mat_inverse(&to_frame).ok_or_else(|| bad(cmd, "the new parent's frame is not invertible"))?;
    let transform = solidcraft_doc::mat_mul(&inverse, &world);
    if !solidcraft_doc::valid_transfer_matrix(&transform) {
        return Err(bad(cmd, "the move exceeds the supported rigid coordinate range"));
    }
    drop(doc);
    let old_path: Vec<u64> = from_path.iter().copied().chain([id]).collect();
    let new_path: Vec<u64> = to_path.iter().copied().chain([id]).collect();

    let mut scratch = s.scratch();
    // Pending moves along both parents and of the placement itself are kept: its new transform
    // was worked out from them.
    for ancestor in from_path.iter().chain(&to_path) {
        if let Some(pending) = scratch.pending_moves.remove(ancestor)
            && let Some(x) = scratch.doc_mut().occurrences.iter_mut().find(|x| x.id == *ancestor)
        {
            x.transform = pending;
        }
    }
    scratch.pending_moves.remove(&id);
    if let Some(x) = scratch.doc_mut().occurrences.iter_mut().find(|x| x.id == id) {
        x.parent = to;
        x.transform = transform;
    }
    // Once all its placements sit in the new parent, the component belongs there too, so what
    // follows the component tree (the parent's eye, its bodies) takes it along.
    if scratch.doc.occurrences.iter().filter(|x| x.component == o.component).all(|x| x.parent == to)
        && let Some(c) = scratch.doc_mut().components.iter_mut().find(|c| c.id == o.component)
    {
        c.parent = to;
    }
    scratch.refresh();
    for f in &scratch.doc.features {
        let broke = scratch.model.result(f.id).and_then(|r| r.error.as_ref());
        let was_fine = s.model.result(f.id).is_none_or(|r| r.error.is_none());
        if let Some(error) = broke.filter(|_| was_fine) {
            return Err(bad(cmd, format!("moving {} would break {}: {error}", o.name, f.name)));
        }
    }
    for sel in &mut scratch.selection {
        if let Sel::Occurrence { path } = sel
            && let Some(moved) = rebased(path, &old_path, &new_path)
        {
            *path = moved;
        }
    }
    if let Some(active) = scratch.active_occurrence_path.as_deref().and_then(|path| rebased(path, &old_path, &new_path)) {
        scratch.active_occurrence_path = Some(active);
    }
    s.accept_computed(scratch);
    for hidden in &mut s.visibility.hidden_occurrences {
        if let Some(moved) = rebased(hidden, &old_path, &new_path) {
            *hidden = moved;
        }
    }
    let (old_scope, new_scope) = (Session::folder_scope(&old_path), Session::folder_scope(&new_path));
    for key in &mut s.visibility.hidden_folders {
        if let Some(rest) = key.strip_prefix(&old_scope).filter(|rest| rest.starts_with('/')) {
            *key = format!("{new_scope}{rest}");
        }
    }
    Ok(json!({"occurrence": id, "parent": to, "path": new_path, "transform": transform}))
}

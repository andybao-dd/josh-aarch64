extern crate crypto;
extern crate git2;
extern crate tracing;

use self::tracing::{warn, Level};
use super::empty_tree_id;
use super::view_maps;
use super::views;
use super::UnapplyView;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

fn all_equal(a: git2::Parents, b: &[&git2::Commit]) -> bool {
    let a: Vec<_> = a.collect();
    if a.len() != b.len() {
        return false;
    }

    for (x, y) in b.iter().zip(a.iter()) {
        if x.id() != y.id() {
            return false;
        }
    }
    return true;
}

// takes everything from base except it's tree and replaces it with the tree
// given
pub fn rewrite(
    repo: &git2::Repository,
    base: &git2::Commit,
    parents: &[&git2::Commit],
    tree: &git2::Tree,
) -> super::JoshResult<git2::Oid> {
    if base.tree()?.id() == tree.id() && all_equal(base.parents(), parents) {
        // Looks like an optimization, but in fact serves to not change the commit in case
        // it was signed.
        return Ok(base.id());
    }

    return Ok(repo.commit(
        None,
        &base.author(),
        &base.committer(),
        &base.message_raw().unwrap_or("no message"),
        tree,
        parents,
    )?);
}

pub fn find_all_views(reference: &git2::Reference) -> HashSet<String> {
    let mut hs = HashSet::new();
    let tree = ok_or!(reference.peel_to_tree(), {
        warn!("find_all_views, not a tree: {:?}", &reference.name());
        return hs;
    });
    ok_or!(
        tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
            if entry.name() == Some(&"workspace.josh") {
                hs.insert(format!(":workspace={}", root.trim_matches('/')));
            }
            if root == "" {
                return 0;
            }
            let v = format!(":/{}", root.trim_matches('/'));
            if v.chars().filter(|x| *x == '/').count() < 5 {
                hs.insert(v);
            }

            0
        }),
        {
            return hs;
        }
    );
    return hs;
}

pub fn unapply_view(
    repo: &git2::Repository,
    backward_maps: std::sync::Arc<std::sync::RwLock<view_maps::ViewMaps>>,
    viewobj: &dyn views::Filter,
    old: git2::Oid,
    new: git2::Oid,
) -> super::JoshResult<UnapplyView> {
    let trace_s = tracing::span!( Level::DEBUG, "unapply_view", repo = ?repo.path(), ?old, ?new);
    let _e = trace_s.enter();

    let walk = {
        let mut walk = repo.revwalk()?;
        walk.set_sorting(git2::Sort::REVERSE | git2::Sort::TOPOLOGICAL)?;
        walk.push(new)?;
        if let Ok(_) = walk.hide(old) {
            tracing::trace!("walk: hidden {}", old);
        } else {
            tracing::warn!("walk: can't hide");
        }
        walk
    };

    let mut bm = view_maps::ViewMaps::new_downstream(backward_maps.clone());
    let mut ret = bm.get(&viewobj.filter_spec(), new);
    for rev in walk {
        let rev = rev?;

        tracing::trace!("==== walking commit {}", rev);

        let module_commit = repo.find_commit(rev)?;

        let original_parent_ids: Vec<_> = module_commit.parent_ids().collect();

        let original_parents: std::result::Result<Vec<_>, _> =
            original_parent_ids
                .iter()
                .map(|x| bm.get(&viewobj.filter_spec(), *x))
                .map(|x| repo.find_commit(x))
                .collect();

        let original_parents = original_parents?;

        let original_parents_refs: Vec<&_> = original_parents.iter().collect();

        tracing::trace!("==== Rewriting commit {}", rev);

        let tree = module_commit.tree()?;

        let new_trees: super::JoshResult<HashSet<_>> = original_parents_refs
            .iter()
            .map(|x| -> super::JoshResult<_> {
                Ok(viewobj.unapply(&repo, &tree, &x.tree()?))
            })
            .collect();

        let new_trees = new_trees?;

        let new_tree = match new_trees.len() {
            1 => repo.find_tree(
                *new_trees
                    .iter()
                    .next()
                    .ok_or(super::josh_error("iter.next"))?,
            )?,
            0 => repo
                // 0 means the history is unrelated. Pushing it will fail if we are not
                // dealing with either a force push or a push with the "josh-merge" option set.
                .find_tree(viewobj.unapply(
                    &repo,
                    &tree,
                    &repo.find_tree(empty_tree_id())?,
                ))?,
            parent_count => {
                // This is a merge commit where the parents in the upstream repo
                // have differences outside of the current view.
                // It is unclear what base tree to pick in this case.
                warn!("rejecting merge");
                return Ok(UnapplyView::RejectMerge(parent_count));
            }
        };

        ret =
            rewrite(&repo, &module_commit, &original_parents_refs, &new_tree)?;
        bm.set(&viewobj.filter_spec(), module_commit.id(), ret);
    }

    return Ok(UnapplyView::Done(ret));
}

pub fn new(path: &Path) -> git2::Repository {
    git2::Repository::init_bare(&path).expect("could not init scratch")
}

fn transform_commit(
    repo: &git2::Repository,
    viewobj: &dyn views::Filter,
    from_refsname: &str,
    to_refname: &str,
    forward_maps: &mut view_maps::ViewMaps,
    backward_maps: &mut view_maps::ViewMaps,
) -> usize {
    let mut updated_count = 0;
    if let Ok(reference) = repo.revparse_single(&from_refsname) {
        let original_commit = ok_or!(reference.peel_to_commit(), {
            warn!("transform_commit, not a commit: {}", from_refsname);
            return updated_count;
        });
        let view_commit = ok_or!(
            viewobj.apply_to_commit(
                &repo,
                &original_commit,
                forward_maps,
                backward_maps,
                &mut HashMap::new(),
            ),
            {
                tracing::error!(
                    "transform_commit, cannot apply_to_commit: {}",
                    from_refsname
                );
                return updated_count;
            }
        );
        forward_maps.set(
            &viewobj.filter_spec(),
            original_commit.id(),
            view_commit,
        );
        backward_maps.set(
            &viewobj.filter_spec(),
            view_commit,
            original_commit.id(),
        );

        let previous = repo
            .revparse_single(&to_refname)
            .map(|x| x.id())
            .unwrap_or(git2::Oid::zero());

        if view_commit != previous {
            updated_count += 1;
            tracing::trace!("transform_commit: update reference: {:?} -> {:?}, target: {:?}, view: {:?}",
                &from_refsname,
                &to_refname,
                view_commit,
                &viewobj.filter_spec());
        }

        if view_commit != git2::Oid::zero() {
            ok_or!(
                repo.reference(&to_refname, view_commit, true, "apply_view")
                    .map(|_| ()),
                {
                    tracing::error!("can't update reference: {:?} -> {:?}, target: {:?}, view: {:?}",
                        &from_refsname,
                        &to_refname,
                        view_commit,
                        &viewobj.filter_spec());
                }
            );
        }
    } else {
        warn!(
            "transform_commit: Can't find reference {:?}",
            &from_refsname
        );
    };
    return updated_count;
}

pub fn apply_view_to_refs(
    repo: &git2::Repository,
    viewobj: &dyn views::Filter,
    refs: &[(String, String)],
    forward_maps: &mut view_maps::ViewMaps,
    backward_maps: &mut view_maps::ViewMaps,
) -> usize {
    tracing::span!(
        Level::TRACE,
        "apply_view_to_refs",
        repo = ?repo.path(),
        ?refs,
        filter_spec=?viewobj.filter_spec());

    let mut updated_count = 0;
    for (k, v) in refs {
        updated_count += transform_commit(
            &repo,
            &*viewobj,
            &k,
            &v,
            forward_maps,
            backward_maps,
        );
    }
    return updated_count;
}

pub fn apply_view_cached(
    repo: &git2::Repository,
    view: &dyn views::Filter,
    newrev: git2::Oid,
    forward_maps: &mut view_maps::ViewMaps,
    backward_maps: &mut view_maps::ViewMaps,
) -> super::JoshResult<git2::Oid> {
    if forward_maps.has(repo, &view.filter_spec(), newrev) {
        return Ok(forward_maps.get(&view.filter_spec(), newrev));
    }

    let trace_s = tracing::span!(Level::TRACE, "apply_view_cached", filter_spec = ?view.filter_spec());

    let walk = {
        let mut walk = repo.revwalk()?;
        walk.set_sorting(git2::Sort::REVERSE | git2::Sort::TOPOLOGICAL)?;
        walk.push(newrev)?;
        walk
    };

    let mut in_commit_count = 0;
    let mut out_commit_count = 0;
    let mut empty_tree_count = 0;
    for original_commit_id in walk {
        in_commit_count += 1;

        let original_commit = repo.find_commit(original_commit_id?)?;

        let filtered_commit = ok_or!(
            view.apply_to_commit(
                &repo,
                &original_commit,
                forward_maps,
                backward_maps,
                &mut HashMap::new(),
            ),
            {
                tracing::error!("cannot apply_to_commit");
                git2::Oid::zero()
            }
        );

        if filtered_commit == git2::Oid::zero() {
            empty_tree_count += 1;
        }
        forward_maps.set(
            &view.filter_spec(),
            original_commit.id(),
            filtered_commit,
        );
        backward_maps.set(
            &view.filter_spec(),
            filtered_commit,
            original_commit.id(),
        );
        out_commit_count += 1;
    }

    if !forward_maps.has(&repo, &view.filter_spec(), newrev) {
        forward_maps.set(&view.filter_spec(), newrev, git2::Oid::zero());
    }
    let rewritten = forward_maps.get(&view.filter_spec(), newrev);
    tracing::event!(
        parent: &trace_s,
        Level::TRACE,
        ?in_commit_count,
        ?out_commit_count,
        ?empty_tree_count,
        original = ?newrev.to_string(),
        rewritten = ?rewritten.to_string(),
    );
    return Ok(rewritten);
}

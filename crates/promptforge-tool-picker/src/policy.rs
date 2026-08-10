//! Turning a ranking into one of four borrowing answers.
//!
//! Ranking says how well each tool matched; this module decides what that is
//! worth and reports it by borrowing the picker's descriptors, so a query never
//! deep-clones a schema.

use crate::catalog::{ToolAnnotations, ToolDescriptor};
use crate::config::Config;
use crate::rank::{Candidate, Vectors, comparable};

/// What the engine concluded about a need, borrowing the picker's descriptors.
///
/// Exactly one variant is produced for a given catalog, need, and
/// configuration. The decision precedence is absent, duplicate, bind,
/// ambiguous, including the solo-candidate rule before the strict-floor
/// abstention, and every threshold boundary is inclusive.
///
/// # The four answers
///
/// [`Outcome::Absent`] is a successful abstention: nothing cleared the floor.
/// [`Outcome::Bind`] is a single tool that cleared the floor and left the
/// runner-up behind by at least the margin. [`Outcome::Duplicate`] reports a
/// group of at least two same-server twins - a fault in one server's catalog.
/// [`Outcome::Ambiguous`] reports every other near-tie the margin could not
/// separate, most often one tool republished across two servers.
///
/// # The solo-candidate rule
///
/// When the leader is at or above [`Config::solo_floor`](crate::Config::solo_floor),
/// below [`Config::similarity_floor`](crate::Config::similarity_floor), and no
/// runner-up reaches the solo floor, the leader binds because there is nothing
/// to confuse it with. Two such candidates abstain.
///
/// # Determinism
///
/// The same model bytes, dependency versions, target, execution environment,
/// catalog, configuration, and need always yield the same outcome.
///
/// # Examples
///
/// ```no_run
/// use promptforge_tool_picker::{Catalog, Config, Outcome, ToolPicker};
///
/// let picker = ToolPicker::build(Catalog::default(), Config::default())?;
/// match picker.resolve("read a file from disk")? {
///     Outcome::Bind(tool) => println!("call {}", tool.name()),
///     Outcome::Duplicate(group) => println!("{} publishes twins", group.first().server()),
///     Outcome::Ambiguous(group) => println!("{} tools fit", group.len()),
///     Outcome::Absent => println!("no tool covers this need"),
///     _ => {}
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Outcome<'a> {
    /// One tool matched clearly enough to be used without asking.
    Bind(&'a ToolDescriptor),
    /// One server publishes tools that are copies of each other.
    Duplicate(CandidateGroup<'a>),
    /// Several tools match well enough that the margin could not separate them.
    Ambiguous(CandidateGroup<'a>),
    /// Nothing in the catalog matched the need well enough to offer.
    Absent,
}

/// A group of at least two candidate descriptors, in ranked order.
///
/// Has no public constructor and always contains at least two entries.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateGroup<'a> {
    /// The group members, best first; the invariant is length two or more.
    tools: Vec<&'a ToolDescriptor>,
}

impl<'a> CandidateGroup<'a> {
    /// Builds a group; the caller guarantees at least two members.
    fn new(tools: Vec<&'a ToolDescriptor>) -> Self {
        debug_assert!(tools.len() >= 2, "a candidate group holds at least two");
        Self { tools }
    }

    /// Returns the number of candidates in the group.
    #[must_use]
    #[expect(
        clippy::len_without_is_empty,
        reason = "a candidate group always holds at least two candidates"
    )]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns the leading candidate.
    #[must_use]
    pub fn first(&self) -> &'a ToolDescriptor {
        self.tools[0]
    }

    /// Returns the second candidate.
    #[must_use]
    pub fn second(&self) -> &'a ToolDescriptor {
        self.tools[1]
    }

    /// Returns the candidate at `index`, if any.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&'a ToolDescriptor> {
        self.tools.get(index).copied()
    }

    /// Iterates the candidates, best first.
    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> CandidateIter<'a, '_> {
        CandidateIter {
            inner: self.tools.iter(),
        }
    }
}

impl<'a, 'group> IntoIterator for &'group CandidateGroup<'a> {
    type Item = &'a ToolDescriptor;
    type IntoIter = CandidateIter<'a, 'group>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A borrowing iterator over a [`CandidateGroup`] or [`Shortlist`].
#[derive(Debug, Clone)]
pub struct CandidateIter<'a, 'group> {
    /// The backing slice iterator over borrowed descriptors.
    inner: std::slice::Iter<'group, &'a ToolDescriptor>,
}

impl<'a> Iterator for CandidateIter<'a, '_> {
    type Item = &'a ToolDescriptor;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().copied()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl DoubleEndedIterator for CandidateIter<'_, '_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back().copied()
    }
}

impl ExactSizeIterator for CandidateIter<'_, '_> {}

impl std::iter::FusedIterator for CandidateIter<'_, '_> {}

/// The candidates a shortlist offers, best first, borrowing the picker.
#[derive(Debug, Clone, PartialEq)]
pub struct Shortlist<'a> {
    /// The offered descriptors, best first; may be empty.
    tools: Vec<&'a ToolDescriptor>,
}

impl<'a> Shortlist<'a> {
    /// The number of candidates offered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the shortlist offers nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// The leading candidate, if any.
    #[must_use]
    pub fn first(&self) -> Option<&'a ToolDescriptor> {
        self.tools.first().copied()
    }

    /// The candidate at `index`, if any.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&'a ToolDescriptor> {
        self.tools.get(index).copied()
    }

    /// Iterates the offered candidates, best first.
    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> CandidateIter<'a, '_> {
        CandidateIter {
            inner: self.tools.iter(),
        }
    }
}

impl<'a, 'list> IntoIterator for &'list Shortlist<'a> {
    type Item = &'a ToolDescriptor;
    type IntoIter = CandidateIter<'a, 'list>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// One ranked candidate resolved back to the tool it names.
#[derive(Debug, Clone, Copy)]
struct Ranked<'a> {
    /// The tool this candidate names.
    tool: &'a ToolDescriptor,
    /// Its position in the catalog: the final tie-break.
    index: usize,
    /// Its cosine similarity to the need.
    score: f32,
}

/// Decides a need from its ranking, the catalog, the vectors, and the config.
pub(crate) fn decide<'a>(
    candidates: &[Candidate],
    tools: &'a [ToolDescriptor],
    vectors: Vectors<'_>,
    config: &Config,
) -> Outcome<'a> {
    let ranked = order(candidates, tools);
    let Some(leader) = ranked.first().copied() else {
        return Outcome::Absent;
    };

    if leader.score < config.similarity_floor() {
        if leader.score >= config.solo_floor() {
            let has_peer = ranked
                .get(1)
                .is_some_and(|runner| runner.score >= config.solo_floor());
            if !has_peer {
                return Outcome::Bind(leader.tool);
            }
        }
        return Outcome::Absent;
    }

    let twins: Vec<Ranked<'a>> = std::iter::once(leader)
        .chain(ranked[1..].iter().copied().filter(|candidate| {
            candidate.tool.server() == leader.tool.server()
                && vectors
                    .similarity(leader.index, candidate.index)
                    .is_some_and(|similarity| similarity >= config.duplicate_threshold())
        }))
        .take(shortlist_bound(config))
        .collect();
    if twins.len() >= 2 {
        return Outcome::Duplicate(CandidateGroup::new(descriptors(&twins)));
    }

    let tied: Vec<Ranked<'a>> = std::iter::once(leader)
        .chain(
            ranked[1..]
                .iter()
                .take_while(|candidate| {
                    candidate.score >= config.similarity_floor()
                        && leader.score - candidate.score < config.margin()
                })
                .copied(),
        )
        .take(shortlist_bound(config))
        .collect();
    if tied.len() < 2 {
        return Outcome::Bind(leader.tool);
    }

    Outcome::Ambiguous(CandidateGroup::new(descriptors(&tied)))
}

/// The candidates worth offering, best first, under the deciding order.
pub(crate) fn shortlist<'a>(
    candidates: &[Candidate],
    tools: &'a [ToolDescriptor],
    config: &Config,
) -> Shortlist<'a> {
    let ranked = order(candidates, tools);
    let above_floor: Vec<&'a ToolDescriptor> = ranked
        .iter()
        .filter(|candidate| candidate.score >= config.similarity_floor())
        .map(|candidate| candidate.tool)
        .collect();

    if !above_floor.is_empty() {
        return Shortlist { tools: above_floor };
    }

    let solo = ranked
        .first()
        .filter(|leader| leader.score >= config.solo_floor())
        .filter(|_| {
            !ranked
                .get(1)
                .is_some_and(|runner| runner.score >= config.solo_floor())
        })
        .map(|leader| leader.tool);

    Shortlist {
        tools: solo.into_iter().collect(),
    }
}

/// How many candidates a shortlist may carry: `top_k`, but never fewer than two.
fn shortlist_bound(config: &Config) -> usize {
    config.top_k().get().max(2)
}

/// The candidates paired with their tools, under the deciding order.
fn order<'a>(candidates: &[Candidate], tools: &'a [ToolDescriptor]) -> Vec<Ranked<'a>> {
    let mut ranked: Vec<Ranked<'a>> = candidates
        .iter()
        .filter_map(|candidate| {
            tools.get(candidate.index()).map(|tool| Ranked {
                tool,
                index: candidate.index(),
                score: candidate.score(),
            })
        })
        .collect();

    ranked.sort_by(|a, b| {
        comparable(b.score)
            .total_cmp(&comparable(a.score))
            .then_with(|| hint_key(a.tool.annotations()).cmp(&hint_key(b.tool.annotations())))
            .then(a.index.cmp(&b.index))
    });
    ranked
}

/// A candidate's hints as a sort key, lower being preferred.
fn hint_key(annotations: ToolAnnotations) -> (u8, u8, u8) {
    (
        u8::from(annotations.read_only() != Some(true)),
        u8::from(annotations.destructive() != Some(false)),
        u8::from(annotations.idempotent() != Some(true)),
    )
}

/// The borrowed descriptors of a group of candidates, in ranked order.
fn descriptors<'a>(group: &[Ranked<'a>]) -> Vec<&'a ToolDescriptor> {
    group.iter().map(|candidate| candidate.tool).collect()
}

#[cfg(test)]
mod tests;

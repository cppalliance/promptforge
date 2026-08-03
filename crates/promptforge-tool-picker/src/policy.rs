//! Turning a ranking into one of four answers.
//!
//! Ranking says how well each tool matched; this module decides what that is
//! worth. The judgement it carries out - what each answer means, in what
//! order the checks apply, where every boundary falls, and how a tie is
//! broken - is documented on [`Outcome`], since that contract is what a
//! caller reads.

use crate::catalog::{ToolAnnotations, ToolDescriptor};
use crate::config::Config;
use crate::rank::{Candidate, Vectors, comparable};

/// What the engine concluded about a need.
///
/// Exactly one variant is produced for a given catalog, need, and
/// configuration: whether the best match is good enough to bind, whether the
/// leaders are too alike to separate, and whether an unseparable pair is a
/// fault in the catalog or a fact about it. The thresholds that decide all of
/// this live in [`Config`], so re-tuning the judgement never touches the
/// arithmetic that produced the scores.
///
/// # The four answers
///
/// [`Outcome::Absent`] is an abstention: nothing in the catalog reached the
/// similarity floor, so the caller is told that rather than handed a
/// nearest-miss it did not ask for. [`Outcome::Bind`] is a single tool, chosen
/// because it cleared the floor and left the runner-up behind by at least the
/// configured margin. The remaining two are both near-ties, and they differ in
/// whether the caller can do anything about it.
///
/// [`Outcome::Duplicate`] reports two or more tools that are copies of one
/// another *and* are served by the same server. One server publishing two
/// tools that no query can tell apart is a fault in that server's catalog, and
/// the server's operator is the caller or someone the caller can reach, so
/// this outcome exists to fail loudly rather than to pick one twin and hope.
/// [`Outcome::Ambiguous`] reports a near-tie the margin could not separate and
/// that was not attributed to one server's own copies - most typically the
/// same underlying tool republished by two servers, which is the expected
/// result of deliberately combining overlapping catalogs. Nothing is wrong, so
/// nothing fails: the engine hands back a short list and lets a caller with
/// fuller context choose.
///
/// # A twin is a property of two tools, not of a query
///
/// `duplicate_threshold` is compared against the cosine similarity between two
/// tools' *own* stored vectors. That is a fact about the pair, the same
/// whatever need is being resolved, and at the default `0.98` it means the two
/// descriptions are near-verbatim republications of one another. It is
/// deliberately not a comparison of the two candidates' query scores: two
/// tools can both score highly against one need while describing quite
/// different capabilities, and two verbatim copies remain copies at every
/// score. Both stored rows are unit length, so their dot product is that
/// cosine with no magnitudes to divide out.
///
/// Nothing therefore relates `duplicate_threshold` to `similarity_floor`. The
/// floor is a query-to-tool measure that admits candidates; the duplicate
/// threshold is a tool-to-tool measure applied among the admitted leader's
/// neighbours. Ordering one against the other would compare two different
/// quantities, so [`Config::validate`] does not.
///
/// # Same server versus different servers is the whole distinction
///
/// A [`ToolDescriptor`] carries the server it came from and nothing that marks
/// that server as the caller's own rather than imported. So the definition in
/// force here is exactly the one the data supports: a group of twins drawn
/// from a single server is a [`Outcome::Duplicate`], and any other group the
/// margin could not separate is [`Outcome::Ambiguous`]. That is what makes the
/// split useful. A single server's catalog is a thing somebody owns and can
/// fix, so naming it is actionable; a collision between two servers is a
/// property of the union the caller chose to assemble, so reporting it as an
/// error would be reporting the caller's own intent back to them as a fault.
///
/// # Precedence
///
/// The checks apply in this order, and the first one that matches is the
/// answer:
///
/// 1. **Absent** - the top score is below `similarity_floor`. Nothing else is
///    asked, because if nothing in the catalog fits the need then whether the
///    two things that do not fit resemble each other is beside the point.
/// 2. **Duplicate** - at least one other candidate shares the leader's server
///    and has a stored vector at or above `duplicate_threshold` similar to the
///    leader's. This is ahead of the margin test on purpose: the outcome
///    exists to fail loudly, and two tools that are copies of each other are a
///    fault whether or not a narrow configured margin happens to separate the
///    scores this particular need gave them.
/// 3. **Bind** - the leader is separated from the runner-up by at least
///    `margin`, or there is no runner-up above the floor. A clear winner wins,
///    and a runner-up that also clears the floor does not weaken it: the floor
///    admits candidates, the margin is what picks between them.
/// 4. **Ambiguous** - everything else, which is a near-tie the margin could
///    not separate and step 2 did not attribute to one server's own copies.
///    The shortlist is the answer of last resort, and it is a shortlist rather
///    than a failure because such ambiguity is ordinary.
///
/// Steps 2 and 4 differ in which group they report, because they are asking
/// different questions. The duplicate group is the leader together with the
/// candidates that share its server and are twins of it, since only those are
/// the fault being reported; it is not filtered by score, because twin-ness
/// does not depend on one. The ambiguous group is the leading candidates that
/// clear the floor and that `margin` failed to separate from the leader, since
/// those are the candidates a caller still has to choose between.
///
/// # Where the boundaries fall
///
/// Every comparison is inclusive, which keeps a configured number readable as
/// "this value is enough":
///
/// - A score *exactly* at `similarity_floor` clears it and is considered.
/// - A gap *exactly* equal to `margin` separates the leader from the
///   runner-up and binds. A `margin` of zero therefore binds on any leader
///   above the floor, including one that ties exactly.
/// - Two stored vectors whose similarity is *exactly* `duplicate_threshold`
///   are twins.
///
/// # Hints break ties, and only ties
///
/// The [`ToolAnnotations`] hints reorder candidates whose scores are *exactly*
/// equal, taking the place of the catalog position that would otherwise decide
/// such a tie; catalog position remains the final key, so the order is still
/// total and a decision is still a function of its inputs. Exactly-equal
/// scores are not a contrivance: a tool republished verbatim under two servers
/// yields identical text, identical vectors, and identical scores.
///
/// The reordering reaches only the candidates it is given. A ranking arrives
/// already cut to length, and the cut is made under score and then catalog
/// position alone, before any hint is read; a hint-preferred tool that tied
/// its way to a position past the cut is therefore absent from the ranking and
/// cannot be promoted into it - it is dropped, not demoted. Hints reorder the
/// exact ties that survived truncation, not every exact tie in the catalog.
/// Ranking more candidates than the shortlist will report is what widens that
/// window.
///
/// Among exactly-tied candidates, the preference is read-only first,
/// non-destructive next, idempotent last, and the rule is that a *positive*
/// claim promotes: a candidate whose `read_only` is `Some(true)` is preferred
/// to one where it is anything else, then a candidate whose `destructive` is
/// `Some(false)` to one where it is anything else, then a candidate whose
/// `idempotent` is `Some(true)` to one where it is anything else. Candidates
/// carrying equal hints, or no hints, are left in catalog order, so a catalog
/// without annotations decides exactly as it would if the hints did not exist.
///
/// An absent hint is deliberately treated as "no claim" rather than as a value
/// to compare: consulting a hint only when both candidates carry it would make
/// the comparison intransitive - a read-only tool would beat a non-read-only
/// one while neither beat a tool that says nothing - and an intransitive
/// comparison has no single correct sorted order, which would cost exactly the
/// determinism the engine promises. Reading silence as the weaker claim is also
/// the cautious reading: a tool that does not say it is read-only has not said
/// it is.
///
/// Hints never overturn a decision the scores made. They cannot promote a
/// near-tie to a [`Outcome::Bind`], and they cannot rescue a
/// [`Outcome::Duplicate`] - a server's twin tools are a fault to report even
/// when one of them is the safer of the two.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// One tool matched clearly enough to be used without asking.
    Bind(ToolDescriptor),
    /// One server publishes tools that are copies of each other.
    ///
    /// A fault in that server's catalog rather than an answer to the need, and
    /// reported so it can be fixed. The list holds the leader and its twins on
    /// the same server, best first. Twin-ness is measured between the tools'
    /// own vectors, not between their scores for this need.
    Duplicate(Vec<ToolDescriptor>),
    /// Several tools match well enough that the margin could not separate them.
    ///
    /// The list holds the candidates the margin failed to separate from the
    /// leader, best first, for a caller with more context to choose from. It
    /// is the residual answer for every near-tie, whatever servers the
    /// candidates came from: the ordinary case is one tool republished across
    /// two catalogs, but two merely similar tools on a single server land here
    /// too. A same-server subset that is indistinguishable in the stronger
    /// sense - copies of one another - is reported as [`Outcome::Duplicate`]
    /// instead, since that is a fault somebody can fix.
    Ambiguous(Vec<ToolDescriptor>),
    /// Nothing in the catalog matched the need well enough to offer.
    Absent,
}

/// One ranked candidate resolved back to the tool it names.
///
/// Pairing the score with the descriptor is what lets the decision ask about
/// a candidate's server and hints, which a bare [`Candidate`] cannot answer.
#[derive(Debug, Clone, Copy)]
struct Ranked<'a> {
    /// The tool this candidate names.
    tool: &'a ToolDescriptor,
    /// Its position in the catalog: the final tie-break, and the key its
    /// stored vector is looked up by.
    index: usize,
    /// Its cosine similarity to the need.
    score: f32,
}

/// Decides a need from its ranking, the catalog, the vectors, and the config.
///
/// `candidates` is a ranking best-first, `tools` is the catalog the candidate
/// positions index, `vectors` holds the stored row for each of those positions
/// so the duplicate check can compare two tools to each other, and `config`
/// supplies the thresholds. The precedence, the boundary conditions, and the
/// tie-break are [`Outcome`]'s contract.
///
/// The decision is a pure function of its four arguments: it reads no clock,
/// no environment, and no global state, and it orders candidates under a total
/// order, so repeating the call yields the same outcome.
///
/// An empty ranking yields [`Outcome::Absent`], as does one whose candidates
/// all point past the end of `tools`; such a candidate is ignored rather than
/// panicked on, and cannot arise from a ranking taken over this catalog. A
/// candidate whose row is missing from `vectors` is likewise never a twin,
/// since the similarity that would make it one cannot be computed.
pub(crate) fn decide(
    candidates: &[Candidate],
    tools: &[ToolDescriptor],
    vectors: Vectors<'_>,
    config: &Config,
) -> Outcome {
    let ranked = order(candidates, tools);
    let Some(leader) = ranked.first().copied() else {
        return Outcome::Absent;
    };

    if leader.score < config.similarity_floor {
        return Outcome::Absent;
    }

    let twins: Vec<Ranked<'_>> = std::iter::once(leader)
        .chain(ranked[1..].iter().copied().filter(|candidate| {
            candidate.tool.server() == leader.tool.server()
                && vectors
                    .similarity(leader.index, candidate.index)
                    .is_some_and(|similarity| similarity >= config.duplicate_threshold)
        }))
        .take(shortlist_bound(config))
        .collect();
    if twins.len() >= 2 {
        return Outcome::Duplicate(descriptors(&twins));
    }

    let tied: Vec<Ranked<'_>> = std::iter::once(leader)
        .chain(
            ranked[1..]
                .iter()
                .take_while(|candidate| {
                    candidate.score >= config.similarity_floor
                        && leader.score - candidate.score < config.margin
                })
                .copied(),
        )
        .take(shortlist_bound(config))
        .collect();
    if tied.len() < 2 {
        return Outcome::Bind(leader.tool.clone());
    }

    Outcome::Ambiguous(descriptors(&tied))
}

/// The candidates worth offering, best first, under the deciding order.
///
/// The same ranking [`decide`] would read, ordered the same way and cut by the
/// same floor, but reported rather than judged: no margin is applied, no twin
/// is looked for, and nothing is collapsed to a single answer. A caller that
/// wants to choose for itself is handed exactly the candidates the engine
/// would have chosen among.
///
/// A candidate scoring below `similarity_floor` is left out, so this yields
/// nothing in precisely the cases [`decide`] answers [`Outcome::Absent`]. The
/// floor is the one threshold that says whether a tool matches the need at
/// all; the margin and the duplicate threshold only say which of the matches
/// wins, and neither question is asked here.
///
/// The list is as long as the ranking it is given, less whatever the floor
/// removed. Cutting it to a length is the caller's business, since the caller
/// chose how many candidates to rank.
pub(crate) fn shortlist(
    candidates: &[Candidate],
    tools: &[ToolDescriptor],
    config: &Config,
) -> Vec<ToolDescriptor> {
    order(candidates, tools)
        .into_iter()
        .filter(|candidate| candidate.score >= config.similarity_floor)
        .map(|candidate| candidate.tool.clone())
        .collect()
}

/// How many candidates a shortlist may carry.
///
/// `top_k`, but never fewer than two. A shortlist of one would name a single
/// tool the engine had explicitly declined to choose, which reads as a binding
/// and is not one; carrying both sides of the tie is the least a report of a
/// tie can say. A configuration is free to ask for a `top_k` of one - it is a
/// valid ranking length - so the floor is applied here rather than rejected
/// there.
fn shortlist_bound(config: &Config) -> usize {
    config.top_k.max(2)
}

/// The candidates paired with their tools, under the deciding order.
///
/// The order is the ranking's own - score descending, with a non-finite score
/// ranking below every real one - refined so that *exactly* tied scores are
/// ordered by their behavioural hints and then by catalog position. Each key is
/// a total order over a value every candidate has, so the composition is total
/// and the sorted result does not depend on the order the candidates arrived
/// in.
fn order<'a>(candidates: &[Candidate], tools: &'a [ToolDescriptor]) -> Vec<Ranked<'a>> {
    let mut ranked: Vec<Ranked<'a>> = candidates
        .iter()
        .filter_map(|candidate| {
            tools.get(candidate.index).map(|tool| Ranked {
                tool,
                index: candidate.index,
                score: candidate.score,
            })
        })
        .collect();

    ranked.sort_by(|a, b| {
        comparable(b.score)
            .total_cmp(&comparable(a.score))
            .then_with(|| hint_key(a.tool.annotations).cmp(&hint_key(b.tool.annotations)))
            .then(a.index.cmp(&b.index))
    });
    ranked
}

/// A candidate's hints as a sort key, lower being preferred.
///
/// Read-only, then non-destructive, then idempotent, each contributing `0` when
/// the tool positively claims the property and `1` when it claims otherwise or
/// says nothing at all. Two candidates that carry the same hints, or none, key
/// identically and are therefore left in catalog order.
fn hint_key(annotations: ToolAnnotations) -> (u8, u8, u8) {
    (
        u8::from(annotations.read_only != Some(true)),
        u8::from(annotations.destructive != Some(false)),
        u8::from(annotations.idempotent != Some(true)),
    )
}

/// The descriptors of a group of candidates, cloned in their ranked order.
fn descriptors(group: &[Ranked<'_>]) -> Vec<ToolDescriptor> {
    group
        .iter()
        .map(|candidate| candidate.tool.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Outcome, decide};
    use crate::catalog::{ToolAnnotations, ToolDescriptor, ToolId};
    use crate::config::Config;
    use crate::rank::{Candidate, Vectors};
    use serde_json::json;

    /// How wide one synthetic stored row is.
    ///
    /// Two dimensions, so every similarity between two rows is exact and
    /// hand-checkable and no model has to be loaded to compute one.
    const STRIDE: usize = 2;

    /// Unit rows a half-radian apart, the closest pair scoring about `0.88`.
    ///
    /// Far enough apart that no pair reaches any duplicate threshold used in
    /// this module, so a catalog of them holds no twins.
    const SPREAD: [f32; 8] = [
        1.0,
        0.0,
        0.877_582_6,
        0.479_425_54,
        0.540_302_3,
        0.841_471,
        0.070_737_2,
        0.997_495,
    ];

    /// One unit row repeated: every pair is a copy, similarity exactly `1.0`.
    const COPIES: [f32; 8] = [0.6, 0.8, 0.6, 0.8, 0.6, 0.8, 0.6, 0.8];

    /// Stored rows for `count` tools that resemble one another as little as
    /// two dimensions allow.
    fn distinct(count: usize) -> &'static [f32] {
        &SPREAD[..count * STRIDE]
    }

    /// Stored rows for `count` tools that are verbatim copies of each other.
    fn twinned(count: usize) -> &'static [f32] {
        &COPIES[..count * STRIDE]
    }

    /// Two rows whose similarity is *exactly* `similarity`.
    ///
    /// The first row is the first basis vector, so the dot product is the
    /// second row's first component and nothing else: exact by construction,
    /// not by luck, which is what a boundary test needs.
    fn pair_at(similarity: f32) -> [f32; 4] {
        [1.0, 0.0, similarity, (1.0 - similarity * similarity).sqrt()]
    }

    /// The rows read as the stored vectors of a catalog.
    fn rows(data: &[f32]) -> Vectors<'_> {
        Vectors::new(data, STRIDE)
    }

    /// A descriptor with the given server and name and nothing else of note.
    fn tool(server: &str, name: &str) -> ToolDescriptor {
        ToolDescriptor::new(ToolId::new(server, name), "does a thing", json!({}))
    }

    /// A descriptor carrying behavioural hints.
    fn hinted(server: &str, name: &str, annotations: ToolAnnotations) -> ToolDescriptor {
        tool(server, name).with_annotations(annotations)
    }

    /// A ranking over the given scores, best first, indexed from zero.
    ///
    /// Synthetic rather than embedded: a score written down is a score at an
    /// exact boundary, and no model has to be loaded to assert what the policy
    /// does with it.
    fn ranking(scores: &[f32]) -> Vec<Candidate> {
        scores
            .iter()
            .enumerate()
            .map(|(index, &score)| Candidate { index, score })
            .collect()
    }

    /// Two tools on one server, and two on two servers, for the tie cases.
    fn one_server() -> Vec<ToolDescriptor> {
        vec![tool("files", "read_file"), tool("files", "load_file")]
    }

    /// The same two tools, republished so the pair straddles two servers.
    fn two_servers() -> Vec<ToolDescriptor> {
        vec![tool("files", "read_file"), tool("blobs", "read_file")]
    }

    /// The next `f32` below `value`, for testing the wrong side of a boundary.
    fn just_below(value: f32) -> f32 {
        f32::from_bits(value.to_bits() - 1)
    }

    /// A configuration whose thresholds are all exactly representable.
    ///
    /// Eighths and quarters, so a difference of scores is exact and a test
    /// that means to sit *on* a boundary is not left a rounding error away
    /// from it.
    fn exact_config() -> Config {
        Config {
            similarity_floor: 0.5,
            margin: 0.125,
            duplicate_threshold: 0.9375,
            ..Config::default()
        }
    }

    #[test]
    fn nothing_above_the_floor_is_an_abstention() {
        let outcome = decide(
            &ranking(&[0.8, 0.4]),
            &two_servers(),
            rows(distinct(2)),
            &Config::default(),
        );
        assert_eq!(outcome, Outcome::Absent);
    }

    #[test]
    fn an_empty_ranking_is_an_abstention() {
        assert_eq!(
            decide(&[], &two_servers(), rows(distinct(2)), &Config::default()),
            Outcome::Absent
        );
    }

    #[test]
    fn a_candidate_pointing_past_the_catalog_is_ignored() {
        let outcome = decide(
            &ranking(&[0.0, 0.0, 0.99])[2..],
            &two_servers(),
            rows(distinct(2)),
            &Config::default(),
        );
        assert_eq!(outcome, Outcome::Absent);
    }

    #[test]
    fn twins_below_the_floor_abstain_rather_than_report_a_duplicate() {
        // The floor is checked before twin-ness, so a pair that no query
        // should have been offered is not offered as a fault either.
        let config = Config {
            similarity_floor: 0.9,
            ..Config::default()
        };
        assert_eq!(
            decide(
                &ranking(&[0.85, 0.85]),
                &one_server(),
                rows(twinned(2)),
                &config
            ),
            Outcome::Absent
        );
        assert_eq!(
            decide(
                &ranking(&[0.5, 0.5]),
                &one_server(),
                rows(twinned(2)),
                &config
            ),
            Outcome::Absent
        );
    }

    #[test]
    fn a_clear_leader_binds() {
        let tools = two_servers();
        let outcome = decide(
            &ranking(&[0.95, 0.7]),
            &tools,
            rows(distinct(2)),
            &Config::default(),
        );
        assert_eq!(outcome, Outcome::Bind(tools[0].clone()));
    }

    #[test]
    fn a_lone_candidate_above_the_floor_binds() {
        let tools = two_servers();
        let outcome = decide(
            &ranking(&[0.9]),
            &tools,
            rows(distinct(2)),
            &Config::default(),
        );
        assert_eq!(outcome, Outcome::Bind(tools[0].clone()));
    }

    #[test]
    fn a_runner_up_above_the_floor_does_not_prevent_a_bind() {
        // 0.86 clears the 0.825 floor, and the 0.09 gap still exceeds the
        // 0.05 margin: the floor admits candidates, the margin chooses.
        let tools = two_servers();
        let config = Config::default();
        assert!(0.86 > config.similarity_floor);
        let outcome = decide(&ranking(&[0.95, 0.86]), &tools, rows(distinct(2)), &config);
        assert_eq!(outcome, Outcome::Bind(tools[0].clone()));
    }

    #[test]
    fn twin_tools_on_one_server_are_a_duplicate() {
        let tools = one_server();
        let outcome = decide(
            &ranking(&[0.99, 0.985]),
            &tools,
            rows(twinned(2)),
            &Config::default(),
        );
        assert_eq!(outcome, Outcome::Duplicate(tools.clone()));
    }

    #[test]
    fn twin_tools_across_servers_are_a_shortlist() {
        let tools = two_servers();
        let outcome = decide(
            &ranking(&[0.99, 0.985]),
            &tools,
            rows(twinned(2)),
            &Config::default(),
        );
        assert_eq!(outcome, Outcome::Ambiguous(tools.clone()));
    }

    #[test]
    fn twins_are_measured_between_the_tools_not_between_their_scores() {
        // Neither score comes anywhere near `duplicate_threshold`, but the
        // two tools are copies of each other, which is the fault reported.
        // Read as a score test this pair would be a mere shortlist.
        let config = Config::default();
        assert!(0.9 < config.duplicate_threshold);
        let tools = one_server();
        assert_eq!(
            decide(&ranking(&[0.9, 0.9]), &tools, rows(twinned(2)), &config),
            Outcome::Duplicate(tools.clone())
        );
    }

    #[test]
    fn two_unalike_tools_on_one_server_are_a_shortlist_however_they_score() {
        // Both scores clear `duplicate_threshold`, which read as a score test
        // would make the pair a fault. Their stored vectors are nothing like
        // each other, and those are what the threshold measures.
        let config = Config::default();
        assert!(0.985 > config.duplicate_threshold);
        let tools = one_server();
        assert_eq!(
            decide(&ranking(&[0.99, 0.985]), &tools, rows(distinct(2)), &config),
            Outcome::Ambiguous(tools.clone())
        );
    }

    #[test]
    fn a_candidate_with_no_stored_row_is_never_a_twin() {
        // Only the leader's row is present, so the similarity that would make
        // the runner-up a twin cannot be computed and is not guessed at.
        let tools = one_server();
        assert_eq!(
            decide(
                &ranking(&[0.99, 0.985]),
                &tools,
                rows(twinned(1)),
                &Config::default()
            ),
            Outcome::Ambiguous(tools.clone())
        );
    }

    #[test]
    fn a_foreign_twin_does_not_make_the_leaders_own_twin_a_shortlist() {
        // Three copies of one tool: two on the leader's server, one
        // elsewhere. The reported fault is the leader's server's own pair.
        let tools = vec![
            tool("files", "read_file"),
            tool("files", "load_file"),
            tool("blobs", "read_file"),
        ];
        let outcome = decide(
            &ranking(&[0.99, 0.985, 0.984]),
            &tools,
            rows(twinned(3)),
            &Config::default(),
        );
        assert_eq!(
            outcome,
            Outcome::Duplicate(vec![tools[0].clone(), tools[1].clone()])
        );
    }

    #[test]
    fn a_twin_is_found_wherever_it_sits_in_the_ranking() {
        // The leader's twin is last, behind a better-scoring stranger. The
        // twin check reads the whole ranking rather than a leading run of it,
        // because a twin's score need not be adjacent to the leader's.
        let tools = vec![
            tool("files", "read_file"),
            tool("blobs", "fetch_url"),
            tool("files", "load_file"),
        ];
        let mut stored = twinned(3).to_vec();
        stored[STRIDE..STRIDE * 2].copy_from_slice(&SPREAD[..STRIDE]);
        let outcome = decide(
            &ranking(&[0.99, 0.985, 0.9]),
            &tools,
            rows(&stored),
            &Config::default(),
        );
        assert_eq!(
            outcome,
            Outcome::Duplicate(vec![tools[0].clone(), tools[2].clone()])
        );
    }

    #[test]
    fn a_duplicate_is_reported_even_when_the_margin_would_separate_it() {
        // Precedence: the duplicate check runs before the margin check, so a
        // margin narrow enough to split the two scores does not turn a
        // server's twin pair into a silent binding.
        let config = Config {
            margin: 0.01,
            ..Config::default()
        };
        let tools = one_server();
        let outcome = decide(&ranking(&[0.995, 0.98]), &tools, rows(twinned(2)), &config);
        assert_eq!(outcome, Outcome::Duplicate(tools.clone()));
    }

    #[test]
    fn a_score_exactly_at_the_floor_is_considered() {
        let config = exact_config();
        let tools = two_servers();
        assert_eq!(
            decide(
                &ranking(&[config.similarity_floor]),
                &tools,
                rows(distinct(2)),
                &config
            ),
            Outcome::Bind(tools[0].clone())
        );
        assert_eq!(
            decide(
                &ranking(&[just_below(config.similarity_floor)]),
                &tools,
                rows(distinct(2)),
                &config
            ),
            Outcome::Absent
        );
    }

    #[test]
    fn a_gap_exactly_equal_to_the_margin_binds() {
        let config = exact_config();
        let tools = two_servers();
        // 0.875 - 0.75 is exactly 0.125, the configured margin.
        assert_eq!(
            decide(&ranking(&[0.875, 0.75]), &tools, rows(distinct(2)), &config),
            Outcome::Bind(tools[0].clone())
        );
        // 0.875 - 0.78125 is 0.09375, a hair inside it.
        assert_eq!(
            decide(
                &ranking(&[0.875, 0.78125]),
                &tools,
                rows(distinct(2)),
                &config
            ),
            Outcome::Ambiguous(tools.clone())
        );
    }

    #[test]
    fn a_pair_exactly_at_the_duplicate_threshold_is_a_twin() {
        let config = exact_config();
        let tools = one_server();
        let threshold = config.duplicate_threshold;
        assert_eq!(
            decide(
                &ranking(&[0.9, 0.9]),
                &tools,
                rows(&pair_at(threshold)),
                &config
            ),
            Outcome::Duplicate(tools.clone())
        );
        // A pair a single bit less alike is no longer a twin pair, so the
        // near-tie is reported as a shortlist instead of a fault.
        assert_eq!(
            decide(
                &ranking(&[0.9, 0.9]),
                &tools,
                rows(&pair_at(just_below(threshold))),
                &config
            ),
            Outcome::Ambiguous(tools.clone())
        );
    }

    #[test]
    fn a_shortlist_is_bounded_by_the_configured_length() {
        let tools = vec![
            tool("a", "one"),
            tool("b", "two"),
            tool("c", "three"),
            tool("d", "four"),
        ];
        let scores = ranking(&[0.9, 0.895, 0.89, 0.885]);

        let Outcome::Ambiguous(shortlist) =
            decide(&scores, &tools, rows(distinct(4)), &Config::default())
        else {
            panic!("four candidates within the margin are a shortlist");
        };
        assert_eq!(shortlist, tools[..3].to_vec());

        // Never fewer than both sides of the tie, whatever `top_k` asks for.
        let narrow = Config {
            top_k: 1,
            ..Config::default()
        };
        let Outcome::Ambiguous(shortlist) = decide(&scores, &tools, rows(distinct(4)), &narrow)
        else {
            panic!("a narrow shortlist is still a shortlist");
        };
        assert_eq!(shortlist, tools[..2].to_vec());
    }

    /// A read-only tool and a plainly modifying one, tied exactly.
    fn read_only_against_writer() -> Vec<ToolDescriptor> {
        vec![
            hinted(
                "files",
                "write_file",
                ToolAnnotations {
                    read_only: Some(false),
                    ..ToolAnnotations::default()
                },
            ),
            hinted(
                "blobs",
                "read_file",
                ToolAnnotations {
                    read_only: Some(true),
                    ..ToolAnnotations::default()
                },
            ),
        ]
    }

    #[test]
    fn a_hint_leads_the_shortlist_when_the_scores_tie_exactly() {
        let tools = read_only_against_writer();
        let outcome = decide(
            &ranking(&[0.9, 0.9]),
            &tools,
            rows(distinct(2)),
            &Config::default(),
        );
        assert_eq!(
            outcome,
            Outcome::Ambiguous(vec![tools[1].clone(), tools[0].clone()]),
            "the read-only tool leads despite coming second in the catalog"
        );
    }

    #[test]
    fn a_hint_chooses_which_tool_binds() {
        // A margin of zero separates even an exact tie, so the binding is the
        // first candidate under the deciding order - which the hints set.
        let config = Config {
            margin: 0.0,
            ..Config::default()
        };
        let tools = read_only_against_writer();
        assert_eq!(
            decide(&ranking(&[0.9, 0.9]), &tools, rows(distinct(2)), &config),
            Outcome::Bind(tools[1].clone())
        );
    }

    #[test]
    fn a_hint_cannot_promote_a_candidate_the_ranking_left_out() {
        // The ranking handed to the decision is already truncated, so a
        // hint-preferred tool that did not survive the cut is simply absent:
        // hints reorder the candidates that arrive, they do not recall one.
        let tools = read_only_against_writer();
        let truncated = &ranking(&[0.9, 0.9])[..1];
        assert_eq!(
            decide(truncated, &tools, rows(distinct(2)), &Config::default()),
            Outcome::Bind(tools[0].clone()),
            "the writer binds because the read-only tool never reached the decision"
        );
    }

    #[test]
    fn a_non_destructive_hint_and_an_idempotent_hint_each_break_a_tie() {
        for (first, second) in [
            (
                ToolAnnotations {
                    destructive: Some(true),
                    ..ToolAnnotations::default()
                },
                ToolAnnotations {
                    destructive: Some(false),
                    ..ToolAnnotations::default()
                },
            ),
            (
                ToolAnnotations {
                    idempotent: Some(false),
                    ..ToolAnnotations::default()
                },
                ToolAnnotations {
                    idempotent: Some(true),
                    ..ToolAnnotations::default()
                },
            ),
        ] {
            let tools = vec![
                hinted("files", "one", first),
                hinted("blobs", "two", second),
            ];
            assert_eq!(
                decide(
                    &ranking(&[0.9, 0.9]),
                    &tools,
                    rows(distinct(2)),
                    &Config::default()
                ),
                Outcome::Ambiguous(vec![tools[1].clone(), tools[0].clone()]),
                "the safer tool leads"
            );
        }
    }

    #[test]
    fn read_only_outranks_the_other_two_hints() {
        let tools = vec![
            hinted(
                "files",
                "one",
                ToolAnnotations {
                    read_only: Some(true),
                    destructive: Some(true),
                    idempotent: Some(false),
                },
            ),
            hinted(
                "blobs",
                "two",
                ToolAnnotations {
                    read_only: Some(false),
                    destructive: Some(false),
                    idempotent: Some(true),
                },
            ),
        ];
        assert_eq!(
            decide(
                &ranking(&[0.9, 0.9]),
                &tools,
                rows(distinct(2)),
                &Config::default()
            ),
            Outcome::Ambiguous(tools.clone()),
            "the read-only claim decides before the other two are consulted"
        );
    }

    #[test]
    fn absent_or_equal_hints_change_nothing() {
        let safe = ToolAnnotations {
            read_only: Some(true),
            destructive: Some(false),
            idempotent: Some(true),
        };
        let unclaimed = ToolAnnotations::default();
        let negative = ToolAnnotations {
            read_only: Some(false),
            destructive: Some(true),
            idempotent: Some(false),
        };

        for (first, second) in [
            (unclaimed, unclaimed),
            (safe, safe),
            (negative, negative),
            // Saying nothing is not a claim, so it neither promotes a tool
            // nor demotes it below one that claims nothing useful.
            (unclaimed, negative),
            (negative, unclaimed),
        ] {
            let tools = vec![
                hinted("files", "one", first),
                hinted("blobs", "two", second),
            ];
            assert_eq!(
                decide(
                    &ranking(&[0.9, 0.9]),
                    &tools,
                    rows(distinct(2)),
                    &Config::default()
                ),
                Outcome::Ambiguous(tools.clone()),
                "catalog order must survive {first:?} against {second:?}"
            );
        }
    }

    #[test]
    fn the_same_input_always_yields_the_same_outcome() {
        let tools = vec![
            hinted(
                "files",
                "read_file",
                ToolAnnotations {
                    read_only: Some(true),
                    ..ToolAnnotations::default()
                },
            ),
            tool("blobs", "read_file"),
            tool("net", "fetch_url"),
        ];
        let cases = [
            ranking(&[0.4, 0.3, 0.2]),
            ranking(&[0.99, 0.4, 0.3]),
            ranking(&[0.99, 0.985, 0.9]),
            ranking(&[0.9, 0.9, 0.9]),
        ];

        for scores in &cases {
            let first = decide(scores, &tools, rows(distinct(3)), &Config::default());
            for _ in 0..16 {
                assert_eq!(
                    decide(scores, &tools, rows(distinct(3)), &Config::default()),
                    first
                );
            }
        }
    }
}

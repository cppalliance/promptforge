The behavior flags a run records for replay.

# Reproducing a run

A run is meant to be reproducible from its log: the same run inputs - the seed and the start instant given to [`RunContext::new`](crate::RunContext::new), and the flags - with the same answers replayed in order produce the same effects and events, each keyed by its [`Provenance`](crate::ids::Provenance). A host records the inputs with the run and the [`EffectRecord`](crate::effect::EffectRecord) and [`AnswerRecord`](crate::effect::AnswerRecord) of every effect it performs.

# Behavior flags

[`Flags`] is how a later engine change that alters a recorded run's behavior stays replayable: the new behavior runs live and sets its flag, and a replay honors the flag only if the original run recorded it. A host records the run's flags ([`RunContext::run_flags`](crate::RunContext::run_flags)) and hands the recorded set back to a replay through [`RunContext::flags`](crate::RunContext::flags).

No flag is defined yet, so every run this engine produces records [`Flags::EMPTY`]. Bit numbering is reserve-forever: a bit is assigned once and never reused, and bits a build does not name survive [`Flags::from_bits`] and [`Flags::bits`], so a record written by a newer engine keeps its flags through an older reader.

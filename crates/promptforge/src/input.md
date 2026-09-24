What a user-input wait is answered with.

# The wait

A section's `user_input()` call issues an [`Effect::UserInput`](crate::effect::Effect::UserInput) naming the run's execution and the asking section. The host answers with [`EffectAnswer::UserInput`](crate::effect::EffectAnswer::UserInput), and the Lua call resumes with two values, the text and whether it is real operator input. The availability flag sits beside the text, so an operator who types exactly the fallback sentence can never spoof the unavailable state. No input tool is advertised to the model: a model loop's scope includes exactly the tools the prompt adds.

# Host policies

The engine knows only the answer vocabulary; the policy is the host's.

- A blocking host parks the wait until the operator delivers, then answers with [`InputOutcome::Text`], delivered byte-exact. The section's VM and message history stay intact while it waits.
- A host with no input to give answers [`InputOutcome::Unavailable`]: the call resolves to a fixed fallback sentence with the flag false, and the prompt continues without input.
- A host whose input source failed answers with an [`InputError`], which raises a typed failure of kind [`RunErrorKind::Input`](crate::RunErrorKind::Input) at the Lua call site. Its message is host-authored and safe to show there; an underlying cause stays behind [`std::error::Error::source`].

The run reports the wait opening and the operator's input as events, so a log shows both without any replay machinery.

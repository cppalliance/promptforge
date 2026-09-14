//! One file per provider: a public `Provider` descriptor plus the
//! private variance of that provider's model-list endpoint - auth header
//! shape, pagination, and response mapping never leave the file.

pub mod anthropic;
pub mod baidu;
pub mod cohere;
pub mod deepgram;
pub mod deepseek;
pub mod elevenlabs;
pub mod gemini;
pub mod groq;
pub mod leonardo;
pub mod meta;
pub mod minimax;
pub mod mistral;
pub mod moonshot;
pub mod openai;
mod openai_shape;
pub mod qwen;
pub mod soniox;
pub mod stepfun;
pub mod xai;

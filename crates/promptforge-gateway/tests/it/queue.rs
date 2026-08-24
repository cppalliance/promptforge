//! Queue admission: dominion concurrency limits and queue-full rejection.

use serde_json::Value;
use tokio::sync::mpsc;

use crate::support::{
    gateway_with_queue, join_within, json_within, next_arrival, slow_fake_backend, spawn_chat,
};

#[tokio::test]
async fn concurrency_one_allows_only_one_in_flight_at_backend() {
    let (backend, mut arrivals) = slow_fake_backend().await;
    let gateway = gateway_with_queue(backend, 1, 10).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/chat/completions", gateway.addr);

    let first = spawn_chat(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    // Second request cannot reach the backend while the first holds the slot.
    let second = spawn_chat(&client, &url);
    assert!(
        matches!(arrivals.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "second request must not reach the backend under concurrency=1"
    );

    release_first.send(()).unwrap();
    assert_eq!(join_within(first).await.unwrap().status().as_u16(), 200);

    // After the first releases, the second is admitted and reaches the backend.
    let release_second = next_arrival(&mut arrivals).await;
    release_second.send(()).unwrap();
    assert_eq!(join_within(second).await.unwrap().status().as_u16(), 200);
    gateway.shutdown().await;
}

#[tokio::test]
async fn concurrency_two_admits_two_in_flight_at_backend() {
    let (backend, mut arrivals) = slow_fake_backend().await;
    let gateway = gateway_with_queue(backend, 2, 10).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/chat/completions", gateway.addr);

    let first = spawn_chat(&client, &url);
    let second = spawn_chat(&client, &url);

    let release_a = next_arrival(&mut arrivals).await;
    let release_b = next_arrival(&mut arrivals).await;

    release_a.send(()).unwrap();
    release_b.send(()).unwrap();
    assert_eq!(join_within(first).await.unwrap().status().as_u16(), 200);
    assert_eq!(join_within(second).await.unwrap().status().as_u16(), 200);
    gateway.shutdown().await;
}

#[tokio::test]
async fn queue_full_returns_503_when_waiting_slots_exhausted() {
    // concurrency=1, max_depth=1: one in-flight + one waiting; the third is 503.
    let (backend, mut arrivals) = slow_fake_backend().await;
    let gateway = gateway_with_queue(backend, 1, 1).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/chat/completions", gateway.addr);

    let first = spawn_chat(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    // Exactly one of these acquires the single waiting slot; the other is 503.
    let mut second = spawn_chat(&client, &url);
    let mut third = spawn_chat(&client, &url);
    let (rejected, survivor) = tokio::select! {
        r = &mut second => (r, third),
        r = &mut third => (r, second),
    };

    let rejected = rejected.unwrap().unwrap();
    assert_eq!(rejected.status().as_u16(), 503);
    let body = json_within(rejected).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("queue_full")
    );

    release_first.send(()).unwrap();
    assert_eq!(join_within(first).await.unwrap().status().as_u16(), 200);

    let release_survivor = next_arrival(&mut arrivals).await;
    release_survivor.send(()).unwrap();
    assert_eq!(join_within(survivor).await.unwrap().status().as_u16(), 200);
    gateway.shutdown().await;
}

use std::{
    collections::HashMap,
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use worldstream_core::{
    Blake3DigestV1, OperationalMmrNodeCoordinateV1, OperationalMmrProofNodeV1,
    OperationalMmrProofV1, OperationalMmrV1,
};

const MEMBER: &[u8] = b"01ARZ3NDEKTSV4RRFFQ69G5FC0";

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    domain: &'static str,
    leaf_count: u64,
    persisted_node_count: usize,
    peak_count: usize,
    logical_node_bytes: usize,
    samples: usize,
    proof_nodes_max: usize,
    proof_nodes_p95: usize,
    proof_vector_allocations_upper_bound: usize,
    verify_micros_p50: u128,
    verify_micros_p95: u128,
    verify_micros_p99: u128,
    build_millis: u128,
    root_hash: String,
    exact_verification_passed: bool,
    tamper_rejected: bool,
}

fn argument(flag: &str) -> Result<String> {
    let args = env::args().collect::<Vec<_>>();
    let position = args
        .iter()
        .position(|value| value == flag)
        .with_context(|| format!("missing {flag}"))?;
    args.get(position + 1)
        .cloned()
        .with_context(|| format!("missing value for {flag}"))
}

fn entry(index: u64) -> Vec<u8> {
    let frame_seq = index.saturating_add(1).to_be_bytes();
    let cause_seq = index.saturating_add(1).to_be_bytes();
    let payload_hash = Blake3DigestV1::hash(&index.to_be_bytes());
    let mut bytes = Vec::with_capacity(96);
    for part in [
        MEMBER,
        frame_seq.as_slice(),
        cause_seq.as_slice(),
        payload_hash.as_bytes().as_slice(),
    ] {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    bytes
}

fn percentile<T: Copy>(values: &[T], numerator: usize, denominator: usize) -> T {
    let index = values.len().saturating_sub(1).saturating_mul(numerator) / denominator;
    values[index]
}

fn main() -> Result<()> {
    let leaf_count = argument("--leaves")?.parse::<u64>()?;
    let requested_samples = argument("--samples")?.parse::<usize>()?;
    let output = PathBuf::from(argument("--output")?);
    if leaf_count == 0 || requested_samples == 0 {
        bail!("leaves and samples must be positive");
    }

    let started = Instant::now();
    let mut mmr = OperationalMmrV1::new("frames")?;
    let mut nodes = HashMap::<(u8, u64), Blake3DigestV1>::new();
    for index in 0..leaf_count {
        let append = mmr.append(&entry(index))?;
        for node in append.nodes() {
            if let Some(existing) =
                nodes.insert((node.height(), node.start_index()), node.digest().clone())
                && existing != node.digest().clone()
            {
                bail!("MMR node coordinate changed");
            }
        }
    }
    let build_millis = started.elapsed().as_millis();
    let samples = requested_samples.min(usize::try_from(leaf_count)?);
    let root = mmr.root();
    let mut proof_counts = Vec::with_capacity(samples);
    let mut latencies = Vec::<Duration>::with_capacity(samples);
    let mut exact = true;
    let mut first_proof = None;
    let mut first_entry = Vec::new();
    for sample in 0..samples {
        let index = if samples == 1 {
            0
        } else {
            u64::try_from(sample)?.saturating_mul(leaf_count.saturating_sub(1))
                / u64::try_from(samples - 1)?
        };
        let plan = mmr.proof_plan(index)?;
        let fetch = |coordinate: &OperationalMmrNodeCoordinateV1| -> Result<_> {
            Ok(OperationalMmrProofNodeV1::new(
                *coordinate,
                nodes
                    .get(&(coordinate.height(), coordinate.start_index()))
                    .with_context(|| {
                        format!(
                            "missing node h={} start={}",
                            coordinate.height(),
                            coordinate.start_index()
                        )
                    })?
                    .clone(),
            ))
        };
        let proof = OperationalMmrProofV1::from_nodes(
            "frames",
            plan.leaf_count(),
            plan.leaf_index(),
            plan.siblings()
                .iter()
                .map(fetch)
                .collect::<Result<Vec<_>>>()?,
            plan.other_peaks()
                .iter()
                .map(fetch)
                .collect::<Result<Vec<_>>>()?,
        )?;
        let canonical_entry = entry(index);
        let verify_started = Instant::now();
        exact &= proof.verify(&canonical_entry, &root);
        latencies.push(verify_started.elapsed());
        proof_counts.push(plan.siblings().len() + plan.other_peaks().len());
        if first_proof.is_none() {
            first_entry = canonical_entry;
            first_proof = Some(proof);
        }
    }
    proof_counts.sort_unstable();
    latencies.sort_unstable();
    let tamper_rejected = !first_proof
        .context("missing proof sample")?
        .verify(&[first_entry, b"tampered".to_vec()].concat(), &root);
    let report = Report {
        schema: "worldstream/operational-mmr-qualification/v1",
        domain: "frames",
        leaf_count,
        persisted_node_count: nodes.len(),
        peak_count: mmr.peaks().len(),
        logical_node_bytes: nodes.len().saturating_mul(1 + 8 + 32),
        samples,
        proof_nodes_max: *proof_counts.last().context("missing proof count")?,
        proof_nodes_p95: percentile(&proof_counts, 95, 100),
        proof_vector_allocations_upper_bound: 2,
        verify_micros_p50: percentile(&latencies, 50, 100).as_micros(),
        verify_micros_p95: percentile(&latencies, 95, 100).as_micros(),
        verify_micros_p99: percentile(&latencies, 99, 100).as_micros(),
        build_millis,
        root_hash: root.to_string(),
        exact_verification_passed: exact,
        tamper_rejected,
    };
    fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    if !exact || !tamper_rejected {
        bail!("MMR qualification failed");
    }
    Ok(())
}

//! Compact authenticated inclusion proofs for retained operational rows.
//!
//! This is a binary-forest Merkle mountain range (MMR). The accumulator keeps
//! only the current peaks, so append memory is logarithmic in the number of
//! leaves. Adapters persist the nodes returned by [`OperationalMmrV1::append`]
//! and use [`OperationalMmrV1::proof_plan`] to fetch a logarithmic proof.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Blake3DigestV1;

const HASH_DOMAIN: &[u8] = b"worldstream/operational-mmr/v1\0";
const MAX_DOMAIN_BYTES: usize = 128;
const MAX_PROOF_NODES: usize = 128;

/// One immutable node in an operational MMR.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalMmrNodeV1 {
    height: u8,
    start_index: u64,
    digest: Blake3DigestV1,
}

/// Compact, canonical trust receipt carried by a checkpoint witness.
///
/// Peaks are included because Core must advance the receipt while replaying a
/// bounded post-checkpoint tail. The root is repeated so storage can compare a
/// serving proof without rebuilding the peak bag on every read.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalMmrReceiptV1 {
    domain: String,
    leaf_count: u64,
    root_hash: Blake3DigestV1,
    peaks: Vec<OperationalMmrNodeV1>,
}

impl OperationalMmrReceiptV1 {
    /// Constructs and validates a persisted receipt.
    pub fn new(
        domain: impl Into<String>,
        leaf_count: u64,
        root_hash: Blake3DigestV1,
        peaks: Vec<OperationalMmrNodeV1>,
    ) -> Result<Self, OperationalMmrErrorV1> {
        let accumulator = OperationalMmrV1::from_peaks(domain, leaf_count, peaks)?;
        if accumulator.root() != root_hash {
            return Err(OperationalMmrErrorV1::RootMismatch);
        }
        Ok(Self::from_accumulator(&accumulator))
    }

    /// Creates the canonical empty receipt for a domain.
    pub fn empty(domain: impl Into<String>) -> Result<Self, OperationalMmrErrorV1> {
        OperationalMmrV1::new(domain).map(|value| Self::from_accumulator(&value))
    }

    /// Captures the current logarithmic accumulator state.
    #[must_use]
    pub fn from_accumulator(accumulator: &OperationalMmrV1) -> Self {
        Self {
            domain: accumulator.domain.clone(),
            leaf_count: accumulator.leaf_count,
            root_hash: accumulator.root(),
            peaks: accumulator.peaks.clone(),
        }
    }

    /// Restores a validated accumulator for a bounded tail append.
    pub fn accumulator(&self) -> Result<OperationalMmrV1, OperationalMmrErrorV1> {
        let accumulator =
            OperationalMmrV1::from_peaks(self.domain.clone(), self.leaf_count, self.peaks.clone())?;
        if accumulator.root() != self.root_hash {
            return Err(OperationalMmrErrorV1::RootMismatch);
        }
        Ok(accumulator)
    }

    #[must_use]
    pub fn domain(&self) -> &str {
        &self.domain
    }

    #[must_use]
    pub const fn leaf_count(&self) -> u64 {
        self.leaf_count
    }

    #[must_use]
    pub const fn root_hash(&self) -> &Blake3DigestV1 {
        &self.root_hash
    }

    #[must_use]
    pub fn peaks(&self) -> &[OperationalMmrNodeV1] {
        &self.peaks
    }
}

impl OperationalMmrNodeV1 {
    /// Computes the immutable height-zero node for one exact canonical row.
    ///
    /// Full storage verifiers use this to compare retained row bytes with the
    /// leaf commitment before rebuilding the complete node inventory.
    pub fn from_canonical_leaf(
        domain: impl Into<String>,
        leaf_index: u64,
        canonical_leaf: &[u8],
    ) -> Result<Self, OperationalMmrErrorV1> {
        let domain = validated_domain(domain.into())?;
        Ok(Self {
            height: 0,
            start_index: leaf_index,
            digest: hash_leaf(&domain, leaf_index, canonical_leaf),
        })
    }

    /// Restores one persisted node after validating its coordinate alignment.
    pub fn new(
        height: u8,
        start_index: u64,
        digest: Blake3DigestV1,
    ) -> Result<Self, OperationalMmrErrorV1> {
        if height >= 64 || start_index % node_width(height) != 0 {
            return Err(OperationalMmrErrorV1::MalformedNode);
        }
        Ok(Self {
            height,
            start_index,
            digest,
        })
    }

    #[must_use]
    pub const fn height(&self) -> u8 {
        self.height
    }

    #[must_use]
    pub const fn start_index(&self) -> u64 {
        self.start_index
    }

    #[must_use]
    pub const fn digest(&self) -> &Blake3DigestV1 {
        &self.digest
    }
}

/// Nodes created by one atomic append, including the new leaf.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationalMmrAppendV1 {
    leaf_index: u64,
    nodes: Vec<OperationalMmrNodeV1>,
    root: Blake3DigestV1,
}

impl OperationalMmrAppendV1 {
    #[must_use]
    pub const fn leaf_index(&self) -> u64 {
        self.leaf_index
    }

    #[must_use]
    pub fn nodes(&self) -> &[OperationalMmrNodeV1] {
        &self.nodes
    }

    #[must_use]
    pub const fn root(&self) -> &Blake3DigestV1 {
        &self.root
    }
}

/// Stable database coordinate for one MMR node.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OperationalMmrNodeCoordinateV1 {
    height: u8,
    start_index: u64,
}

impl OperationalMmrNodeCoordinateV1 {
    #[must_use]
    pub const fn new(height: u8, start_index: u64) -> Self {
        Self {
            height,
            start_index,
        }
    }

    #[must_use]
    pub const fn height(self) -> u8 {
        self.height
    }

    #[must_use]
    pub const fn start_index(self) -> u64 {
        self.start_index
    }
}

/// Coordinates an adapter must fetch to assemble an inclusion proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationalMmrProofPlanV1 {
    leaf_count: u64,
    leaf_index: u64,
    siblings: Vec<OperationalMmrNodeCoordinateV1>,
    other_peaks: Vec<OperationalMmrNodeCoordinateV1>,
}

impl OperationalMmrProofPlanV1 {
    /// Derives the exact logarithmic node inventory for a retained leaf.
    pub fn new(leaf_count: u64, leaf_index: u64) -> Result<Self, OperationalMmrErrorV1> {
        proof_plan_for_count(leaf_count, leaf_index)
    }

    #[must_use]
    pub const fn leaf_count(&self) -> u64 {
        self.leaf_count
    }

    #[must_use]
    pub const fn leaf_index(&self) -> u64 {
        self.leaf_index
    }

    #[must_use]
    pub fn siblings(&self) -> &[OperationalMmrNodeCoordinateV1] {
        &self.siblings
    }

    #[must_use]
    pub fn other_peaks(&self) -> &[OperationalMmrNodeCoordinateV1] {
        &self.other_peaks
    }
}

/// A proof node and its exact coordinate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationalMmrProofNodeV1 {
    coordinate: OperationalMmrNodeCoordinateV1,
    digest: Blake3DigestV1,
}

impl OperationalMmrProofNodeV1 {
    #[must_use]
    pub const fn new(coordinate: OperationalMmrNodeCoordinateV1, digest: Blake3DigestV1) -> Self {
        Self { coordinate, digest }
    }

    #[must_use]
    pub const fn coordinate(&self) -> OperationalMmrNodeCoordinateV1 {
        self.coordinate
    }

    #[must_use]
    pub const fn digest(&self) -> &Blake3DigestV1 {
        &self.digest
    }
}

/// A bounded inclusion proof assembled from persisted MMR nodes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationalMmrProofV1 {
    domain: String,
    leaf_count: u64,
    leaf_index: u64,
    siblings: Vec<OperationalMmrProofNodeV1>,
    other_peaks: Vec<OperationalMmrProofNodeV1>,
}

impl OperationalMmrProofV1 {
    /// Constructs a proof only when every supplied node matches the canonical
    /// plan for `leaf_count` and `leaf_index`.
    pub fn from_nodes(
        domain: impl Into<String>,
        leaf_count: u64,
        leaf_index: u64,
        siblings: Vec<OperationalMmrProofNodeV1>,
        other_peaks: Vec<OperationalMmrProofNodeV1>,
    ) -> Result<Self, OperationalMmrErrorV1> {
        let domain = validated_domain(domain.into())?;
        let plan = proof_plan_for_count(leaf_count, leaf_index)?;
        if coordinates(&siblings) != plan.siblings || coordinates(&other_peaks) != plan.other_peaks
        {
            return Err(OperationalMmrErrorV1::MalformedProof);
        }
        Ok(Self {
            domain,
            leaf_count,
            leaf_index,
            siblings,
            other_peaks,
        })
    }

    /// Verifies the exact canonical leaf bytes against the trusted root.
    #[must_use]
    pub fn verify(&self, canonical_leaf: &[u8], trusted_root: &Blake3DigestV1) -> bool {
        if self.siblings.len() + self.other_peaks.len() > MAX_PROOF_NODES {
            return false;
        }
        let Ok(plan) = proof_plan_for_count(self.leaf_count, self.leaf_index) else {
            return false;
        };
        if coordinates(&self.siblings) != plan.siblings
            || coordinates(&self.other_peaks) != plan.other_peaks
        {
            return false;
        }

        let Some(target_peak) = peak_containing(self.leaf_count, self.leaf_index) else {
            return false;
        };
        let mut node = OperationalMmrNodeV1 {
            height: 0,
            start_index: self.leaf_index,
            digest: hash_leaf(&self.domain, self.leaf_index, canonical_leaf),
        };
        for sibling in &self.siblings {
            let coordinate = sibling.coordinate;
            if coordinate.height != node.height {
                return false;
            }
            let width = node_width(node.height);
            let (left, right, parent_start) =
                if coordinate.start_index.checked_add(width) == Some(node.start_index) {
                    (sibling.digest(), node.digest(), coordinate.start_index)
                } else if node.start_index.checked_add(width) == Some(coordinate.start_index) {
                    (node.digest(), sibling.digest(), node.start_index)
                } else {
                    return false;
                };
            let Some(parent_height) = node.height.checked_add(1) else {
                return false;
            };
            node = OperationalMmrNodeV1 {
                height: parent_height,
                start_index: parent_start,
                digest: hash_parent(&self.domain, parent_height, parent_start, left, right),
            };
        }
        if node.height != target_peak.height || node.start_index != target_peak.start_index {
            return false;
        }

        let mut other = self.other_peaks.iter();
        let mut peaks = Vec::with_capacity(self.other_peaks.len() + 1);
        for coordinate in peak_coordinates(self.leaf_count) {
            if coordinate == target_peak {
                peaks.push(node.clone());
            } else {
                let Some(value) = other.next() else {
                    return false;
                };
                peaks.push(OperationalMmrNodeV1 {
                    height: coordinate.height,
                    start_index: coordinate.start_index,
                    digest: value.digest.clone(),
                });
            }
        }
        other.next().is_none() && hash_root(&self.domain, self.leaf_count, &peaks) == *trusted_root
    }
}

/// Logarithmic append state for one Room and operational domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationalMmrV1 {
    domain: String,
    leaf_count: u64,
    peaks: Vec<OperationalMmrNodeV1>,
}

impl OperationalMmrV1 {
    pub fn new(domain: impl Into<String>) -> Result<Self, OperationalMmrErrorV1> {
        Ok(Self {
            domain: validated_domain(domain.into())?,
            leaf_count: 0,
            peaks: Vec::new(),
        })
    }

    /// Restores an accumulator from its bounded durable inventory.
    pub fn from_peaks(
        domain: impl Into<String>,
        leaf_count: u64,
        peaks: Vec<OperationalMmrNodeV1>,
    ) -> Result<Self, OperationalMmrErrorV1> {
        let domain = validated_domain(domain.into())?;
        if peaks.len() > 64 || coordinates(&peaks) != peak_coordinates(leaf_count) {
            return Err(OperationalMmrErrorV1::MalformedPeaks);
        }
        Ok(Self {
            domain,
            leaf_count,
            peaks,
        })
    }

    #[must_use]
    pub fn domain(&self) -> &str {
        &self.domain
    }

    #[must_use]
    pub const fn leaf_count(&self) -> u64 {
        self.leaf_count
    }

    #[must_use]
    pub fn peaks(&self) -> &[OperationalMmrNodeV1] {
        &self.peaks
    }

    #[must_use]
    pub fn root(&self) -> Blake3DigestV1 {
        hash_root(&self.domain, self.leaf_count, &self.peaks)
    }

    /// Appends one exact canonical row while retaining only O(log n) peaks.
    pub fn append(
        &mut self,
        canonical_leaf: &[u8],
    ) -> Result<OperationalMmrAppendV1, OperationalMmrErrorV1> {
        let leaf_index = self.leaf_count;
        let leaf = OperationalMmrNodeV1::from_canonical_leaf(
            self.domain.clone(),
            leaf_index,
            canonical_leaf,
        )?;
        self.append_prehashed_leaf(leaf.digest)
    }

    /// Rebuilds an MMR from an already domain-and-index-bound leaf digest.
    ///
    /// This is reserved for full storage verification after a retained row
    /// has been pruned. Callers must compare every still-retained row with
    /// [`OperationalMmrNodeV1::from_canonical_leaf`] before trusting it.
    pub fn append_prehashed_leaf(
        &mut self,
        leaf_digest: Blake3DigestV1,
    ) -> Result<OperationalMmrAppendV1, OperationalMmrErrorV1> {
        let leaf_index = self.leaf_count;
        let next_count = leaf_index
            .checked_add(1)
            .ok_or(OperationalMmrErrorV1::LeafCountOverflow)?;
        let mut node = OperationalMmrNodeV1 {
            height: 0,
            start_index: leaf_index,
            digest: leaf_digest,
        };
        let mut created = vec![node.clone()];
        while self
            .peaks
            .last()
            .is_some_and(|peak| peak.height == node.height)
        {
            let left = self.peaks.pop().expect("last peak was checked");
            let width = node_width(node.height);
            if left.start_index.checked_add(width) != Some(node.start_index) {
                return Err(OperationalMmrErrorV1::MalformedPeaks);
            }
            let parent_height = node
                .height
                .checked_add(1)
                .ok_or(OperationalMmrErrorV1::LeafCountOverflow)?;
            node = OperationalMmrNodeV1 {
                height: parent_height,
                start_index: left.start_index,
                digest: hash_parent(
                    &self.domain,
                    parent_height,
                    left.start_index,
                    &left.digest,
                    &node.digest,
                ),
            };
            created.push(node.clone());
        }
        self.peaks.push(node);
        self.leaf_count = next_count;
        let root = self.root();
        Ok(OperationalMmrAppendV1 {
            leaf_index,
            nodes: created,
            root,
        })
    }

    pub fn proof_plan(
        &self,
        leaf_index: u64,
    ) -> Result<OperationalMmrProofPlanV1, OperationalMmrErrorV1> {
        proof_plan_for_count(self.leaf_count, leaf_index)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum OperationalMmrErrorV1 {
    #[error("operational MMR domain must contain 1..={MAX_DOMAIN_BYTES} bytes")]
    InvalidDomain,
    #[error("operational MMR leaf count overflow")]
    LeafCountOverflow,
    #[error("operational MMR leaf index is outside the retained range")]
    LeafIndexOutOfRange,
    #[error("operational MMR peak inventory is malformed")]
    MalformedPeaks,
    #[error("operational MMR node coordinate is malformed")]
    MalformedNode,
    #[error("operational MMR proof inventory is malformed")]
    MalformedProof,
    #[error("operational MMR receipt root does not match its peaks")]
    RootMismatch,
}

fn validated_domain(domain: String) -> Result<String, OperationalMmrErrorV1> {
    if domain.is_empty() || domain.len() > MAX_DOMAIN_BYTES {
        return Err(OperationalMmrErrorV1::InvalidDomain);
    }
    Ok(domain)
}

fn coordinates<T>(nodes: &[T]) -> Vec<OperationalMmrNodeCoordinateV1>
where
    T: MmrNodeCoordinate,
{
    nodes.iter().map(MmrNodeCoordinate::coordinate).collect()
}

trait MmrNodeCoordinate {
    fn coordinate(&self) -> OperationalMmrNodeCoordinateV1;
}

impl MmrNodeCoordinate for OperationalMmrNodeV1 {
    fn coordinate(&self) -> OperationalMmrNodeCoordinateV1 {
        OperationalMmrNodeCoordinateV1::new(self.height, self.start_index)
    }
}

impl MmrNodeCoordinate for OperationalMmrProofNodeV1 {
    fn coordinate(&self) -> OperationalMmrNodeCoordinateV1 {
        self.coordinate
    }
}

fn proof_plan_for_count(
    leaf_count: u64,
    leaf_index: u64,
) -> Result<OperationalMmrProofPlanV1, OperationalMmrErrorV1> {
    if leaf_index >= leaf_count {
        return Err(OperationalMmrErrorV1::LeafIndexOutOfRange);
    }
    let target_peak =
        peak_containing(leaf_count, leaf_index).ok_or(OperationalMmrErrorV1::MalformedPeaks)?;
    let mut siblings = Vec::with_capacity(usize::from(target_peak.height));
    for height in 0..target_peak.height {
        let width = node_width(height);
        let local = leaf_index - target_peak.start_index;
        let node_start = target_peak.start_index + (local / width) * width;
        let sibling_start = if ((local / width) & 1) == 0 {
            node_start + width
        } else {
            node_start - width
        };
        siblings.push(OperationalMmrNodeCoordinateV1::new(height, sibling_start));
    }
    let other_peaks = peak_coordinates(leaf_count)
        .into_iter()
        .filter(|coordinate| *coordinate != target_peak)
        .collect();
    Ok(OperationalMmrProofPlanV1 {
        leaf_count,
        leaf_index,
        siblings,
        other_peaks,
    })
}

fn peak_containing(leaf_count: u64, leaf_index: u64) -> Option<OperationalMmrNodeCoordinateV1> {
    peak_coordinates(leaf_count).into_iter().find(|peak| {
        leaf_index >= peak.start_index
            && leaf_index < peak.start_index.saturating_add(node_width(peak.height))
    })
}

fn peak_coordinates(leaf_count: u64) -> Vec<OperationalMmrNodeCoordinateV1> {
    let mut peaks = Vec::with_capacity(leaf_count.count_ones() as usize);
    let mut start = 0_u64;
    for height in (0_u8..64).rev() {
        if leaf_count & node_width(height) != 0 {
            peaks.push(OperationalMmrNodeCoordinateV1::new(height, start));
            start = start.saturating_add(node_width(height));
        }
    }
    peaks
}

const fn node_width(height: u8) -> u64 {
    1_u64 << height
}

fn hash_leaf(domain: &str, leaf_index: u64, canonical_leaf: &[u8]) -> Blake3DigestV1 {
    tagged_hash(&[
        b"leaf",
        domain.as_bytes(),
        &leaf_index.to_be_bytes(),
        canonical_leaf,
    ])
}

fn hash_parent(
    domain: &str,
    height: u8,
    start_index: u64,
    left: &Blake3DigestV1,
    right: &Blake3DigestV1,
) -> Blake3DigestV1 {
    tagged_hash(&[
        b"node",
        domain.as_bytes(),
        &[height],
        &start_index.to_be_bytes(),
        left.as_bytes(),
        right.as_bytes(),
    ])
}

fn hash_root(domain: &str, leaf_count: u64, peaks: &[OperationalMmrNodeV1]) -> Blake3DigestV1 {
    let mut hasher = blake3::Hasher::new();
    hash_part(&mut hasher, HASH_DOMAIN);
    hash_part(&mut hasher, b"root");
    hash_part(&mut hasher, domain.as_bytes());
    hash_part(&mut hasher, &leaf_count.to_be_bytes());
    hash_part(&mut hasher, &(peaks.len() as u64).to_be_bytes());
    for peak in peaks {
        hash_part(&mut hasher, &[peak.height]);
        hash_part(&mut hasher, &peak.start_index.to_be_bytes());
        hash_part(&mut hasher, peak.digest.as_bytes());
    }
    Blake3DigestV1::from_bytes(*hasher.finalize().as_bytes())
}

fn tagged_hash(parts: &[&[u8]]) -> Blake3DigestV1 {
    let mut hasher = blake3::Hasher::new();
    hash_part(&mut hasher, HASH_DOMAIN);
    for part in parts {
        hash_part(&mut hasher, part);
    }
    Blake3DigestV1::from_bytes(*hasher.finalize().as_bytes())
}

fn hash_part(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proof(
        mmr: &OperationalMmrV1,
        all_nodes: &[OperationalMmrNodeV1],
        index: u64,
    ) -> OperationalMmrProofV1 {
        let plan = mmr.proof_plan(index).expect("plan");
        let fetch = |coordinate: OperationalMmrNodeCoordinateV1| {
            let node = all_nodes
                .iter()
                .rev()
                .find(|node| node.coordinate() == coordinate)
                .expect("persisted node");
            OperationalMmrProofNodeV1::new(coordinate, node.digest.clone())
        };
        OperationalMmrProofV1::from_nodes(
            "frames",
            plan.leaf_count(),
            plan.leaf_index(),
            plan.siblings().iter().copied().map(fetch).collect(),
            plan.other_peaks().iter().copied().map(fetch).collect(),
        )
        .expect("proof")
    }

    #[test]
    fn every_leaf_verifies_across_uneven_mountains_with_logarithmic_state() {
        let mut mmr = OperationalMmrV1::new("frames").expect("MMR");
        let leaves: Vec<Vec<u8>> = (0_u64..1_037)
            .map(|index| format!("frame-{index}").into_bytes())
            .collect();
        let mut nodes = Vec::new();
        for leaf in &leaves {
            nodes.extend(mmr.append(leaf).expect("append").nodes().iter().cloned());
            assert!(mmr.peaks().len() <= 64);
        }
        assert_eq!(mmr.leaf_count(), 1_037);
        for index in [0_u64, 1, 511, 512, 1_023, 1_024, 1_036] {
            let candidate = proof(&mmr, &nodes, index);
            assert!(candidate.verify(&leaves[index as usize], &mmr.root()));
            assert!(candidate.siblings.len() + candidate.other_peaks.len() <= 64);
        }
    }

    #[test]
    fn tampered_leaf_path_domain_index_and_root_fail_closed() {
        let mut mmr = OperationalMmrV1::new("frames").expect("MMR");
        let leaves: Vec<Vec<u8>> = (0_u64..37)
            .map(|index| format!("frame-{index}").into_bytes())
            .collect();
        let mut nodes = Vec::new();
        for leaf in &leaves {
            nodes.extend(mmr.append(leaf).expect("append").nodes().iter().cloned());
        }
        let candidate = proof(&mmr, &nodes, 17);
        assert!(!candidate.verify(b"substituted-frame", &mmr.root()));

        let mut bad_path = candidate.clone();
        bad_path.siblings[0].digest = Blake3DigestV1::hash(b"bad sibling");
        assert!(!bad_path.verify(&leaves[17], &mmr.root()));

        let mut bad_domain = candidate.clone();
        bad_domain.domain = "decisions".to_owned();
        assert!(!bad_domain.verify(&leaves[17], &mmr.root()));

        let mut bad_index = candidate.clone();
        bad_index.leaf_index = 18;
        assert!(!bad_index.verify(&leaves[17], &mmr.root()));

        assert!(!candidate.verify(&leaves[17], &Blake3DigestV1::hash(b"bad root")));
    }

    #[test]
    fn malformed_coordinates_inventory_and_ranges_are_rejected() {
        let mut mmr = OperationalMmrV1::new("frames").expect("MMR");
        let mut nodes = Vec::new();
        for leaf in [b"a".as_slice(), b"b", b"c"] {
            nodes.extend(mmr.append(leaf).expect("append").nodes().iter().cloned());
        }
        assert_eq!(
            mmr.proof_plan(3),
            Err(OperationalMmrErrorV1::LeafIndexOutOfRange)
        );
        let plan = mmr.proof_plan(1).expect("plan");
        let bad = OperationalMmrProofV1::from_nodes(
            "frames",
            3,
            1,
            Vec::new(),
            plan.other_peaks()
                .iter()
                .map(|coordinate| {
                    OperationalMmrProofNodeV1::new(*coordinate, Blake3DigestV1::hash(b"x"))
                })
                .collect(),
        );
        assert_eq!(bad, Err(OperationalMmrErrorV1::MalformedProof));
        assert_eq!(
            OperationalMmrV1::from_peaks("frames", 3, Vec::new()),
            Err(OperationalMmrErrorV1::MalformedPeaks)
        );
        assert_eq!(
            OperationalMmrV1::new(""),
            Err(OperationalMmrErrorV1::InvalidDomain)
        );
        assert_eq!(
            OperationalMmrNodeV1::new(2, 3, Blake3DigestV1::hash(b"x")),
            Err(OperationalMmrErrorV1::MalformedNode)
        );
    }

    #[test]
    fn receipt_round_trips_logarithmic_peaks_and_rejects_a_stale_root() {
        let mut mmr = OperationalMmrV1::new("frames").expect("MMR");
        for index in 0_u64..100_000 {
            mmr.append(&index.to_be_bytes()).expect("append");
        }
        let receipt = OperationalMmrReceiptV1::from_accumulator(&mmr);
        assert_eq!(receipt.leaf_count(), 100_000);
        assert!(receipt.peaks().len() <= 64);
        assert_eq!(receipt.accumulator().expect("receipt"), mmr);
        assert_eq!(
            OperationalMmrReceiptV1::new(
                "frames",
                receipt.leaf_count(),
                Blake3DigestV1::hash(b"stale"),
                receipt.peaks().to_vec(),
            ),
            Err(OperationalMmrErrorV1::RootMismatch)
        );
    }

    #[test]
    fn prehashed_leaf_rebuild_reproduces_every_node_and_receipt() {
        let leaves: Vec<Vec<u8>> = (0_u64..1_037)
            .map(|index| format!("frame-{index}").into_bytes())
            .collect();
        let mut original = OperationalMmrV1::new("frames").expect("original MMR");
        let mut original_nodes = Vec::new();
        for leaf in &leaves {
            original_nodes.extend(
                original
                    .append(leaf)
                    .expect("append")
                    .nodes()
                    .iter()
                    .cloned(),
            );
        }

        let leaf_nodes: Vec<_> = original_nodes
            .iter()
            .filter(|node| node.height() == 0)
            .cloned()
            .collect();
        let mut rebuilt = OperationalMmrV1::new("frames").expect("rebuilt MMR");
        let mut rebuilt_nodes = Vec::new();
        for node in leaf_nodes {
            rebuilt_nodes.extend(
                rebuilt
                    .append_prehashed_leaf(node.digest().clone())
                    .expect("prehashed append")
                    .nodes()
                    .iter()
                    .cloned(),
            );
        }

        assert_eq!(rebuilt_nodes, original_nodes);
        assert_eq!(
            OperationalMmrReceiptV1::from_accumulator(&rebuilt),
            OperationalMmrReceiptV1::from_accumulator(&original)
        );
        assert_eq!(
            OperationalMmrNodeV1::from_canonical_leaf("frames", 17, &leaves[17])
                .expect("canonical leaf"),
            original_nodes
                .iter()
                .find(|node| node.height() == 0 && node.start_index() == 17)
                .expect("stored leaf")
                .clone()
        );
    }
}

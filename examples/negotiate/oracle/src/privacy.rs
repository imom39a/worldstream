use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{PERSONAS, Persona, PrivacyClass, PrivacyExposure};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PrivacyMatrixRow {
    pub class: PrivacyClass,
    pub buyer_agent: PrivacyExposure,
    pub seller_agent: PrivacyExposure,
    pub buyer_approver: PrivacyExposure,
    pub venue_signer: PrivacyExposure,
    pub operator: PrivacyExposure,
    pub spectator: PrivacyExposure,
}

impl PrivacyMatrixRow {
    #[must_use]
    pub const fn exposure(self, persona: Persona) -> PrivacyExposure {
        match persona {
            Persona::BuyerAgent => self.buyer_agent,
            Persona::SellerAgent => self.seller_agent,
            Persona::BuyerApprover => self.buyer_approver,
            Persona::VenueSigner => self.venue_signer,
            Persona::Operator => self.operator,
            Persona::Spectator => self.spectator,
        }
    }
}

pub const PRIVACY_MATRIX: [PrivacyMatrixRow; 8] = [
    PrivacyMatrixRow {
        class: PrivacyClass::ProposalRevision,
        buyer_agent: PrivacyExposure::Full,
        seller_agent: PrivacyExposure::Full,
        buyer_approver: PrivacyExposure::Full,
        venue_signer: PrivacyExposure::Reference,
        operator: PrivacyExposure::Bounded,
        spectator: PrivacyExposure::None,
    },
    PrivacyMatrixRow {
        class: PrivacyClass::CurrentTermDiff,
        buyer_agent: PrivacyExposure::Full,
        seller_agent: PrivacyExposure::Full,
        buyer_approver: PrivacyExposure::Full,
        venue_signer: PrivacyExposure::None,
        operator: PrivacyExposure::Bounded,
        spectator: PrivacyExposure::None,
    },
    PrivacyMatrixRow {
        class: PrivacyClass::BuyerAcceptanceEnvelope,
        buyer_agent: PrivacyExposure::Full,
        seller_agent: PrivacyExposure::None,
        buyer_approver: PrivacyExposure::Full,
        venue_signer: PrivacyExposure::Reference,
        operator: PrivacyExposure::Bounded,
        spectator: PrivacyExposure::None,
    },
    PrivacyMatrixRow {
        class: PrivacyClass::SignedApproval,
        buyer_agent: PrivacyExposure::Full,
        seller_agent: PrivacyExposure::Full,
        buyer_approver: PrivacyExposure::Full,
        venue_signer: PrivacyExposure::Reference,
        operator: PrivacyExposure::Bounded,
        spectator: PrivacyExposure::None,
    },
    PrivacyMatrixRow {
        class: PrivacyClass::SignedStreamEvent,
        buyer_agent: PrivacyExposure::Full,
        seller_agent: PrivacyExposure::Full,
        buyer_approver: PrivacyExposure::Reference,
        venue_signer: PrivacyExposure::Reference,
        operator: PrivacyExposure::Bounded,
        spectator: PrivacyExposure::None,
    },
    PrivacyMatrixRow {
        class: PrivacyClass::CommittedAgreement,
        buyer_agent: PrivacyExposure::Full,
        seller_agent: PrivacyExposure::Full,
        buyer_approver: PrivacyExposure::Full,
        venue_signer: PrivacyExposure::Reference,
        operator: PrivacyExposure::Full,
        spectator: PrivacyExposure::None,
    },
    PrivacyMatrixRow {
        class: PrivacyClass::OperationalDiagnostic,
        buyer_agent: PrivacyExposure::None,
        seller_agent: PrivacyExposure::None,
        buyer_approver: PrivacyExposure::None,
        venue_signer: PrivacyExposure::None,
        operator: PrivacyExposure::Bounded,
        spectator: PrivacyExposure::None,
    },
    PrivacyMatrixRow {
        class: PrivacyClass::PublicStatus,
        buyer_agent: PrivacyExposure::Full,
        seller_agent: PrivacyExposure::Full,
        buyer_approver: PrivacyExposure::Full,
        venue_signer: PrivacyExposure::Full,
        operator: PrivacyExposure::Full,
        spectator: PrivacyExposure::Full,
    },
];

/// Synthetic private inputs used only to prove pairwise noninterference. They
/// do not model Activity State and may contain no strategy or prompt data in a
/// production implementation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PrivacyProbe {
    pub public_status: String,
    pub private_by_persona: BTreeMap<Persona, String>,
}

impl PrivacyProbe {
    #[must_use]
    pub fn fixture() -> Self {
        Self {
            public_status: "formation_open".to_owned(),
            private_by_persona: PERSONAS
                .into_iter()
                .map(|persona| (persona, format!("{}-private-probe", persona.as_str())))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PrivacyProjection {
    pub viewer: Persona,
    pub public_status: String,
    pub own_private_probe: Option<String>,
}

#[must_use]
pub fn privacy_projection(viewer: Persona, probe: &PrivacyProbe) -> PrivacyProjection {
    PrivacyProjection {
        viewer,
        public_status: probe.public_status.clone(),
        own_private_probe: probe.private_by_persona.get(&viewer).cloned(),
    }
}

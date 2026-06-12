//! Knowledge pack definitions and role-to-pack mapping.
//!
//! Each pack is a static, curated text corpus covering a specific domain.
//! Packs are embedded once at first launch and stored in the global knowledge
//! store. At mock-interview start, `packs_for_role` selects the relevant subset.

use std::collections::HashSet;

use uuid::Uuid;

/// A bundled domain knowledge pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackId {
    Algorithms,
    SystemDesign,
    DesignPatterns,
    Behavioral,
    MlPlatform,
    WebBackend,
    Frontend,
}

impl PackId {
    /// Stable UUID used as `session_id` in the global vector store.
    ///
    /// Values are in a reserved namespace (`f17fffff-0000-0000-NNNN-…`) and
    /// will never collide with real v4 session UUIDs.
    pub fn uuid(self) -> Uuid {
        match self {
            Self::Algorithms    => Uuid::from_u128(0xf17f_ffff_0000_0000_0001_0000_0000_0000),
            Self::SystemDesign  => Uuid::from_u128(0xf17f_ffff_0000_0000_0002_0000_0000_0000),
            Self::DesignPatterns=> Uuid::from_u128(0xf17f_ffff_0000_0000_0003_0000_0000_0000),
            Self::Behavioral    => Uuid::from_u128(0xf17f_ffff_0000_0000_0004_0000_0000_0000),
            Self::MlPlatform    => Uuid::from_u128(0xf17f_ffff_0000_0000_0005_0000_0000_0000),
            Self::WebBackend    => Uuid::from_u128(0xf17f_ffff_0000_0000_0006_0000_0000_0000),
            Self::Frontend      => Uuid::from_u128(0xf17f_ffff_0000_0000_0007_0000_0000_0000),
        }
    }

    /// Subdirectory name under the knowledge base root directory.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Algorithms    => "algorithms",
            Self::SystemDesign  => "system_design",
            Self::DesignPatterns=> "design_patterns",
            Self::Behavioral    => "behavioral",
            Self::MlPlatform    => "ml_platform",
            Self::WebBackend    => "web_backend",
            Self::Frontend      => "frontend",
        }
    }

    pub fn all() -> &'static [PackId] {
        &[
            Self::Algorithms,
            Self::SystemDesign,
            Self::DesignPatterns,
            Self::Behavioral,
            Self::MlPlatform,
            Self::WebBackend,
            Self::Frontend,
        ]
    }
}

/// Select relevant knowledge packs based on the session's domain and role.
///
/// Always includes `Algorithms` and `Behavioral`. Domain/role strings are
/// matched case-insensitively; unrecognised roles fall back to a general
/// software-engineering selection.
pub fn packs_for_role(domain: &str, role: &str) -> Vec<PackId> {
    let combined = format!("{domain} {role}").to_lowercase();

    let is_ml = ["ml", "ai ", " ai", "machine learning", "data science", "llm",
                 "nlp", "mlops", "platform engineer", "ai engineer", "deep learning",
                 "computer vision", "recommender"]
        .iter()
        .any(|kw| combined.contains(kw));

    let is_frontend = ["frontend", "front-end", "react", "vue", "angular",
                       "ui engineer", "ux engineer", "css", " html"]
        .iter()
        .any(|kw| combined.contains(kw));

    let is_backend = ["backend", "back-end", "api", "server", "microservice",
                      "cloud", "devops", "infrastructure", "sre", "platform",
                      "distributed"]
        .iter()
        .any(|kw| combined.contains(kw));

    const GENERAL_SW_PACKS: &[PackId] =
        &[PackId::SystemDesign, PackId::WebBackend, PackId::DesignPatterns];

    let mut packs = vec![PackId::Algorithms, PackId::Behavioral];

    if is_ml {
        packs.extend_from_slice(&[PackId::MlPlatform, PackId::SystemDesign, PackId::DesignPatterns]);
    }

    if is_frontend {
        packs.extend_from_slice(&[PackId::Frontend, PackId::WebBackend, PackId::DesignPatterns]);
    }

    if is_backend {
        packs.extend_from_slice(GENERAL_SW_PACKS);
    }

    // Unrecognised role → general software-engineering selection.
    if !is_ml && !is_frontend && !is_backend {
        packs.extend_from_slice(GENERAL_SW_PACKS);
    }

    // Dedup preserving insertion order.
    let mut seen = HashSet::new();
    packs.retain(|p| seen.insert(p.uuid()));
    packs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ml_engineer_gets_ml_and_algo_packs() {
        let packs = packs_for_role("fintech", "AI Engineer");
        assert!(packs.contains(&PackId::MlPlatform));
        assert!(packs.contains(&PackId::Algorithms));
        assert!(packs.contains(&PackId::Behavioral));
    }

    #[test]
    fn frontend_engineer_gets_frontend_packs() {
        let packs = packs_for_role("e-commerce", "React Frontend Engineer");
        assert!(packs.contains(&PackId::Frontend));
        assert!(packs.contains(&PackId::WebBackend));
        assert!(!packs.contains(&PackId::MlPlatform));
    }

    #[test]
    fn backend_engineer_gets_sysdesign_packs() {
        let packs = packs_for_role("saas", "Backend Engineer");
        assert!(packs.contains(&PackId::SystemDesign));
        assert!(packs.contains(&PackId::WebBackend));
    }

    #[test]
    fn no_duplicate_packs_in_result() {
        let packs = packs_for_role("ai platform", "ML Platform Engineer");
        let uuids: HashSet<_> = packs.iter().map(|p| p.uuid()).collect();
        assert_eq!(packs.len(), uuids.len());
    }
}

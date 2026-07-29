use std::fmt;

use mise_core::types::Slug;

use crate::error::StoreError;

/// Identity of one Automerge document in the store. The string form doubles
/// as the SQLite key: `queue`, `location/home/pantry`, `recipe/mapo-tofu`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DocId {
    State,
    Queue,
    Someday,
    Shopping,
    Steering,
    Facts,
    Pantry(Slug),
    Equipment(Slug),
    Shops(Slug),
    Fridge(Slug),
    Recipe(Slug),
    Technique(Slug),
}

impl DocId {
    pub fn kind(&self) -> &'static str {
        match self {
            DocId::State => "state",
            DocId::Queue => "queue",
            DocId::Someday => "someday",
            DocId::Shopping => "shopping",
            DocId::Steering => "steering",
            DocId::Facts => "facts",
            DocId::Pantry(_) => "pantry",
            DocId::Equipment(_) => "equipment",
            DocId::Shops(_) => "shops",
            DocId::Fridge(_) => "fridge",
            DocId::Recipe(_) => "recipe",
            DocId::Technique(_) => "technique",
        }
    }

    /// Where this doc renders in the export tree. Kept honest by a test
    /// against the render map.
    pub fn export_path(&self) -> String {
        match self {
            DocId::State => "state.md".to_string(),
            DocId::Queue => "queue.md".to_string(),
            DocId::Someday => "someday.md".to_string(),
            DocId::Shopping => "shopping.md".to_string(),
            DocId::Steering => "steering.md".to_string(),
            DocId::Facts => "facts.md".to_string(),
            DocId::Pantry(l) => format!("locations/{l}/pantry.md"),
            DocId::Equipment(l) => format!("locations/{l}/equipment.md"),
            DocId::Shops(l) => format!("locations/{l}/shops.md"),
            DocId::Fridge(l) => format!("locations/{l}/fridge.md"),
            DocId::Recipe(r) => format!("recipes/{r}.md"),
            DocId::Technique(t) => format!("techniques/{t}.md"),
        }
    }

    pub fn parse(s: &str) -> Result<DocId, StoreError> {
        let bad = || StoreError::BadDocId(s.to_string());
        match s {
            "state" => return Ok(DocId::State),
            "queue" => return Ok(DocId::Queue),
            "someday" => return Ok(DocId::Someday),
            "shopping" => return Ok(DocId::Shopping),
            "steering" => return Ok(DocId::Steering),
            "facts" => return Ok(DocId::Facts),
            _ => {}
        }
        let parts: Vec<&str> = s.split('/').collect();
        let slug = |raw: &str| Slug::new(raw).map_err(|_| bad());
        match parts.as_slice() {
            ["location", loc, "pantry"] => Ok(DocId::Pantry(slug(loc)?)),
            ["location", loc, "equipment"] => Ok(DocId::Equipment(slug(loc)?)),
            ["location", loc, "shops"] => Ok(DocId::Shops(slug(loc)?)),
            ["location", loc, "fridge"] => Ok(DocId::Fridge(slug(loc)?)),
            ["recipe", r] => Ok(DocId::Recipe(slug(r)?)),
            ["technique", t] => Ok(DocId::Technique(slug(t)?)),
            _ => Err(bad()),
        }
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocId::State => write!(f, "state"),
            DocId::Queue => write!(f, "queue"),
            DocId::Someday => write!(f, "someday"),
            DocId::Shopping => write!(f, "shopping"),
            DocId::Steering => write!(f, "steering"),
            DocId::Facts => write!(f, "facts"),
            DocId::Pantry(l) => write!(f, "location/{l}/pantry"),
            DocId::Equipment(l) => write!(f, "location/{l}/equipment"),
            DocId::Shops(l) => write!(f, "location/{l}/shops"),
            DocId::Fridge(l) => write!(f, "location/{l}/fridge"),
            DocId::Recipe(r) => write!(f, "recipe/{r}"),
            DocId::Technique(t) => write!(f, "technique/{t}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips() {
        for s in [
            "state",
            "queue",
            "someday",
            "shopping",
            "steering",
            "facts",
            "location/home/pantry",
            "location/cottage/fridge",
            "recipe/mapo-tofu",
            "technique/velveting",
        ] {
            assert_eq!(DocId::parse(s).unwrap().to_string(), s);
        }
        for bad in ["", "location/home", "recipe/", "recipe/Bad Slug", "x/y/z"] {
            assert!(DocId::parse(bad).is_err(), "{bad}");
        }
    }
}

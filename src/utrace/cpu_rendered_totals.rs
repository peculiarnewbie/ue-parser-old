//! Aggregate repeated metadata names without allocating keys on cache hits.

use std::collections::BTreeMap;

use rustc_hash::FxHashMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct CpuRenderedScopeTotals {
    specs: FxHashMap<u32, FxHashMap<String, (u64, u64)>>,
}

impl CpuRenderedScopeTotals {
    pub(super) fn record(&mut self, spec_id: u32, name: &str, duration: u64) {
        let names = self.specs.entry(spec_id).or_default();
        if let Some(total) = names.get_mut(name) {
            total.0 += 1;
            total.1 = total.1.saturating_add(duration);
        } else {
            names.insert(name.to_owned(), (1, duration));
        }
    }

    #[cfg(feature = "utrace-parallel")]
    pub(super) fn merge(&mut self, other: Self) {
        for (spec_id, names) in other.specs {
            let target = self.specs.entry(spec_id).or_default();
            for (name, (count, cycles)) in names {
                let total = target.entry(name).or_default();
                total.0 = total.0.saturating_add(count);
                total.1 = total.1.saturating_add(cycles);
            }
        }
    }

    pub(super) fn into_ordered(self) -> BTreeMap<(u32, String), (u64, u64)> {
        self.specs
            .into_iter()
            .flat_map(|(spec, names)| {
                names
                    .into_iter()
                    .map(move |(name, total)| ((spec, name), total))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_shared_within_each_spec_and_output_is_ordered() {
        let mut totals = CpuRenderedScopeTotals::default();
        totals.record(7, "Frame", 20);
        totals.record(7, "Frame", 30);
        totals.record(2, "Frame", 10);
        totals.record(7, "Other", u64::MAX);
        totals.record(7, "Other", 1);
        assert_eq!(totals.specs[&7].len(), 2);
        assert_eq!(
            totals.into_ordered().into_iter().collect::<Vec<_>>(),
            vec![
                ((2, "Frame".to_owned()), (1, 10)),
                ((7, "Frame".to_owned()), (2, 50)),
                ((7, "Other".to_owned()), (2, u64::MAX)),
            ]
        );
    }
}

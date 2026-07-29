//! "You run out of food Thursday." Coverage counts dinners: cooked servings
//! divided by the location's headcount, one dinner per day starting today.
//! Freezer portions are long-tail coverage — "unless you defrost the March
//! bolognese" — reported separately, never silently merged in.

use std::num::NonZeroU32;

use jiff::ToSpan;
use jiff::civil::Date;

use crate::types::Portion;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    /// Whole dinners the fridge covers, starting with today's.
    pub dinners: u32,
    /// First uncovered dinner date, fridge alone.
    pub runs_out: Date,
    /// Additional whole dinners if frozen portions are defrosted.
    pub freezer_dinners: u32,
    /// First uncovered dinner date if the freezer is drawn down too.
    pub runs_out_with_freezer: Date,
}

fn dinners(servings: u64, headcount: NonZeroU32) -> u32 {
    u32::try_from(servings / u64::from(headcount.get())).unwrap_or(u32::MAX)
}

fn total(portions: &[Portion]) -> u64 {
    portions.iter().map(|p| u64::from(p.servings)).sum()
}

pub fn coverage(
    fridge: &[Portion],
    freezer: &[Portion],
    headcount: NonZeroU32,
    today: Date,
) -> Coverage {
    let fridge_dinners = dinners(total(fridge), headcount);
    let all_dinners = dinners(total(fridge) + total(freezer), headcount);
    Coverage {
        dinners: fridge_dinners,
        runs_out: today.saturating_add((i64::from(fridge_dinners)).days()),
        freezer_dinners: all_dinners - fridge_dinners,
        runs_out_with_freezer: today.saturating_add((i64::from(all_dinners)).days()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portion(servings: u32) -> Portion {
        Portion {
            dish: "mapo tofu".into(),
            servings,
            date: Date::constant(2026, 7, 26),
        }
    }

    #[test]
    fn empty_fridge_runs_out_today() {
        let today = Date::constant(2026, 7, 29);
        let c = coverage(&[], &[], NonZeroU32::new(2).unwrap(), today);
        assert_eq!(c.dinners, 0);
        assert_eq!(c.runs_out, today);
        assert_eq!(c.runs_out_with_freezer, today);
    }

    #[test]
    fn partial_dinners_do_not_count() {
        // 5 servings at headcount 2: tonight and tomorrow, then a sad single
        // serving that covers no dinner.
        let today = Date::constant(2026, 7, 29);
        let c = coverage(&[portion(5)], &[], NonZeroU32::new(2).unwrap(), today);
        assert_eq!(c.dinners, 2);
        assert_eq!(c.runs_out, Date::constant(2026, 7, 31));
    }

    #[test]
    fn freezer_extends_but_is_reported_separately() {
        let today = Date::constant(2026, 7, 29);
        let c = coverage(
            &[portion(4)],
            &[portion(4)],
            NonZeroU32::new(2).unwrap(),
            today,
        );
        assert_eq!(c.dinners, 2);
        assert_eq!(c.freezer_dinners, 2);
        assert_eq!(c.runs_out, Date::constant(2026, 7, 31));
        assert_eq!(c.runs_out_with_freezer, Date::constant(2026, 8, 2));
    }

    #[test]
    fn leftover_servings_pool_across_portions() {
        // 1 + 1 servings at headcount 2 make one dinner together.
        let today = Date::constant(2026, 7, 29);
        let c = coverage(
            &[portion(1), portion(1)],
            &[],
            NonZeroU32::new(2).unwrap(),
            today,
        );
        assert_eq!(c.dinners, 1);
    }
}

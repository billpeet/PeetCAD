"use strict";
// Which of an export's equations SolidWorks applies.
//
// The exporter writes IEquationMgr.Disabled as `disabled`, and SolidWorks 2018 gives true for every equation but the
// first, in every document, where SolidWorks applies them all (the jobs' parts have the values they give). Disabling
// an equation takes it out of the list GetCount and Equation[i] read ("you are removing it from the list of
// relations"): GetDisabledEquationCount (format 9's `disabledEquationCount`) counts those, and is as high as 4 where
// one equation is listed (Escutcheon Surround Removable). So every equation listed is in force, unless it is
// suppressed in the configuration (IEquationMgr.Suppression, format 9's `suppressed`).
//
// `settleEquations(doc)` sets each equation's `disabled` to whether SolidWorks does NOT apply it, so the importers'
// `e.disabled` means that; what the API said is kept as `apiDisabled`.

/** Whether SolidWorks applies equation `e` (of a configuration's list), with why not. */
function equationState(e) {
  if (e.suppressed === true) return { applied: false, why: "suppressed in this configuration" };
  return { applied: true };
}

/** Rewrites each equation's `disabled` of an export (parsed) to whether SolidWorks leaves it out. Idempotent. */
function settleEquations(doc) {
  if (!doc || doc.__equationsSettled) return doc;
  for (const cfg of doc.configurations || []) {
    for (const e of cfg.equations || []) {
      if (e.apiDisabled === undefined) e.apiDisabled = e.disabled;
      const s = equationState(e);
      e.disabled = !s.applied;
      if (s.why) e.notApplied = s.why;
    }
  }
  Object.defineProperty(doc, "__equationsSettled", { value: true, enumerable: false });
  return doc;
}

module.exports = { equationState, settleEquations };

"use strict";
// Which of an export's equations SolidWorks applies.
//
// The exporter writes IEquationMgr.Disabled as `disabled`, and in every export SolidWorks 2018 gives it true for
// every equation but the first, where SolidWorks applies them all: GetDisabledEquationCount is 0 and the jobs'
// parts have the values the equations give. From format 9 the exporter writes that count
// (`disabledEquationCount`, per configuration) and IEquationMgr.Suppression (`suppressed`, in that configuration).
// An equation is in force unless it is suppressed in the configuration, or disabled where SolidWorks says some are.
//
// `settleEquations(doc)` sets each equation's `disabled` to whether SolidWorks does NOT apply it, so the importers'
// `e.disabled` means that; what the API said is kept as `apiDisabled`.

/** Whether SolidWorks applies equation `e` of configuration `cfg`, with why not. */
function equationState(e, cfg) {
  if (e.suppressed === true) return { applied: false, why: "suppressed in this configuration" };
  const count = cfg && cfg.disabledEquationCount;
  if (typeof count === "number" && count > 0) {
    // Disabled equations there are: Disabled is believed only if it names as many as SolidWorks counts
    const named = ((cfg && cfg.equations) || []).filter(x => (x.apiDisabled !== undefined ? x.apiDisabled : x.disabled) === true).length;
    const flag = e.apiDisabled !== undefined ? e.apiDisabled : e.disabled;
    if (named === count && flag === true) return { applied: false, why: "disabled" };
  }
  return { applied: true };
}

/** Rewrites each equation's `disabled` of an export (parsed) to whether SolidWorks leaves it out. Idempotent. */
function settleEquations(doc) {
  if (!doc || doc.__equationsSettled) return doc;
  for (const cfg of doc.configurations || []) {
    const list = cfg.equations || [];
    for (const e of list) if (e.apiDisabled === undefined) e.apiDisabled = e.disabled;
    for (const e of list) {
      const s = equationState(e, cfg);
      e.disabled = !s.applied;
      if (s.why) e.notApplied = s.why;
    }
  }
  Object.defineProperty(doc, "__equationsSettled", { value: true, enumerable: false });
  return doc;
}

module.exports = { equationState, settleEquations };

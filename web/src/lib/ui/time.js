// An age, which is the form people read a timestamp in.
//
// "3m" answers "is this happening now"; "2026-09-22T15:04:11Z" does not,
// and a column of them is unreadable at a glance. The exact time stays in
// the title attribute, because the moment you need it you need it exactly.
export function ago(iso) {
  if (!iso) return ''
  const t = Date.parse(iso)
  if (Number.isNaN(t)) return ''
  const s = Math.max(0, Math.round((Date.now() - t) / 1000))
  if (s < 60) return `${s}s`
  if (s < 3600) return `${Math.round(s / 60)}m`
  if (s < 86400) return `${Math.round(s / 3600)}h`
  return `${Math.round(s / 86400)}d`
}

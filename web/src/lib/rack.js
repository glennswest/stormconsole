// Who a node's rack belongs to (#77), apart from the page so a node test can read it. stormipmi sets the label from a machine's location, where it runs
// and has a machine for this node. Anywhere else, an admin sets it by hand. stormipmi leaves a label it did not set alone, so a disagreement is said, not fought over.

/// `machines`: the Machines API's list, or null when stormipmi does not answer here.
export function rackState(host, label, machines) {
  const machine = (machines || []).find((m) => m.node === host) || null
  const recorded = machine?.location?.rack || ''
  if (!machine) return { owner: 'hand', rack: label || '', machine: null, recorded: '', note: '' }
  let note = ''
  if (recorded && label && recorded !== label) note = `${machine.tag || machine.name}'s location says ${recorded}, and stormipmi leaves a label it did not set`
  else if (recorded && !label) note = `${machine.tag || machine.name}'s location says ${recorded}: not on the Node yet`
  return { owner: 'stormipmi', rack: label || '', machine, recorded, note }
}

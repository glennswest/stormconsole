// node web/src/lib/charts.test.mjs
import assert from 'node:assert/strict'
import { chartQuery, chartNode, fromWords, levelHref } from './charts.js'

let n = 0
const t = (name, f) => { f(); n++; console.log(`  ok   ${name}`) }

t('an object on another node names it', () => {
  assert.equal(chartQuery({ entity: 'vm', ns: 'a', name: 'b' }, 'storm-b').toString(), 'entity=vm&ns=a&name=b&on=storm-b')
})
t('no node known: this node’s', () => {
  assert.equal(chartQuery({ entity: 'drive', serial: 'S1' }, '').toString(), 'entity=drive&serial=S1')
  assert.equal(chartQuery('entity=pod&ns=a&name=b', undefined).toString(), 'entity=pod&ns=a&name=b')
})
t('a node already named stays', () => {
  assert.equal(chartQuery('entity=vm&ns=a&name=b&on=storm-c', 'storm-b').get('on'), 'storm-c')
})
t('the Metrics page says whose charts they are', () => {
  assert.equal(chartNode('entity=drive&serial=S1&on=storm-b'), 'storm-b')
  assert.equal(chartNode('entity=port&node=forge1&ifname=eth0'), 'forge1')
  assert.equal(chartNode('entity=raidset&node=forge1&set=md0'), 'forge1', 'a RAID set is on its node (#385)')
  assert.equal(chartNode('entity=member&set=md0&slot=2&on=forge1'), 'forge1')
  assert.equal(chartNode('entity=vm&ns=a&name=b'), '')
  assert.equal(chartNode('level=drives'), '')
  assert.equal(fromWords('storm-b'), 'From storm-b’s ironprom')
  assert.equal(fromWords(''), 'From this node’s ironprom')
})
t('a level is charted for the node it names (#386)', () => {
  assert.equal(levelHref('node', 'storm-b'), '#/metrics?level=node&on=storm-b')
  assert.equal(levelHref('network', ''), '#/metrics?level=network')
  assert.equal(chartNode(levelHref('node', 'storm-b').split('?')[1]), 'storm-b', 'and the page says whose')
  assert.equal(levelHref('node', 'a&level=x'), '#/metrics?level=node&on=a%26level%3Dx', 'a name cannot add a key')
})
console.log(`charts: ${n} ok`)

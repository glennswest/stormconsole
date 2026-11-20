// node web/src/lib/charts.test.mjs
import assert from 'node:assert/strict'
import { chartQuery, chartNode, fromWords } from './charts.js'

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
  assert.equal(chartNode('entity=vm&ns=a&name=b'), '')
  assert.equal(chartNode('level=drives'), '')
  assert.equal(fromWords('storm-b'), 'From storm-b’s ironprom')
  assert.equal(fromWords(''), 'From this node’s ironprom')
})
console.log(`charts: ${n} ok`)

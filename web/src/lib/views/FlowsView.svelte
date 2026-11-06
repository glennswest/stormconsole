<script>
  // Network Flows (#4): in the project picked in the masthead, or every project this viewer may see.
  import { k8sns } from '../stores.svelte.js'
  import PageHeader from '../components/PageHeader.svelte'
  import FlowTable from '../components/FlowTable.svelte'
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Networking' }, { label: 'Network Flows' }]}
    title="Network Flows"
    scope={k8sns.selected ? `in ${k8sns.selected}` : 'every project you may see'}
  />
  <div class="sc-panel" style="padding: 14px var(--sc-row-px)">
    <FlowTable ns={k8sns.selected || ''} />
  </div>
</div>

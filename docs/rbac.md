# The console's apiserver role

The role stormcos ships for the console (stormcos#76 step 2), ready to apply in
[`deploy/rbac/stormconsole.yaml`](../deploy/rbac/stormconsole.yaml). The owner decided its shape on stormcos#203:
not read-only and not cluster-admin, but "a role that may write the settings of the components it manages", with
its writes "listed resource by resource".

`crates/stormconsole/src/rbac.rs` checks the file against the code: a newly watched kind or a new write fails the
build until it is in the role. `deploy/verify-rbac.sh` runs a console with only this role against a real apiserver.

## Two credentials

Every apiserver call goes out on one of two credentials:

- **A signed-in user's own token, when they have one.** The apiserver's RBAC for that user then decides. This role
  does not apply.
- **The console's own token, otherwise.** That covers everything the console does by itself, and every request
  from someone with no kubernetes identity: a node with auth off, the node's bearer, or a client certificate
  (`client.rs` `auth`). **This role is all such a person can do.**

## Reads

All are cluster-wide.

| Group | Resources | Verbs | Why |
|---|---|---|---|
| core | namespaces, nodes, pods, services, persistentvolumeclaims, persistentvolumes, configmaps, resourcequotas, limitranges | get, list, watch | the watched cache |
| apps | deployments, statefulsets, daemonsets | get, list, watch | the watched cache |
| batch | jobs, cronjobs | get, list, watch | the watched cache |
| networking.k8s.io | networkpolicies | get, list, watch | the watched cache |
| storage.k8s.io | storageclasses | get, list, watch | the Cluster section |
| apiextensions.k8s.io | customresourcedefinitions | get, list, watch | the Cluster section |
| rbac.authorization.k8s.io | clusterroles | get, list, watch | the Cluster section |
| route.openshift.io | routes | get, list, watch | a project's contents (optional kind) |
| k8s.cni.cncf.io | network-attachment-definitions | get, list, watch | a project's contents (optional kind) |
| cilium.io | ciliumendpoints, ciliumnodes, ciliumidentities, ciliumnetworkpolicies, ciliumclusterwidenetworkpolicies | get, list, watch | the network pages on a cilium node (optional kinds) |
| flowsdn.io | flowsdnendpoints, flowsdnidentities, flowsdnnetworkpolicies, flowsdnclusterwidenetworkpolicies | get, list, watch | the network pages on a flowsdn node (optional kinds, #168) |
| core | events | get, list | cluster-wide events, filtered to what the viewer may see; object event boxes |
| core | pods/log | get | the last 5 runs per container, kept in the background (#69) |
| core | endpoints | get | a pod page's Services, for someone with no identity |
| apps | replicasets | get | a pod page's owner chain, for someone with no identity |
| project.openshift.io | projects | get, list | the Projects page, for someone with no identity |
| rbac.authorization.k8s.io | rolebindings | get, list | a project's members, for someone with no identity |
| non-resource | `/version` | get | the apiserver probe on the Kubernetes card |

Nothing is needed for the rest:

- **The kubelet** (`/stats/summary`, `/metrics/cadvisor`) only authenticates the token with a TokenReview, so the
  role grants nothing there (rustkube-node `server.rs`).
- **Access reviews** (`selfsubjectaccessreviews`) and the namespace probe are asked as the viewer. Someone with no
  identity is refused before any call is made.

## Writes

These are the settings of the components the console manages, and the whole list.

| Group | Resource | Verbs | What it is |
|---|---|---|---|
| kubevirt.io | virtualmachines | patch | a machine's settings, disks, memory floor, and whether it runs |
| kubevirt.io | virtualmachineinstances | delete | restart and stop: deleting the running instance |
| snapshot.kubevirt.io | virtualmachinesnapshots | create, delete | a machine's snapshots |
| snapshot.kubevirt.io | virtualmachinerestores | create | restoring one |
| cluster.storm.io | clusters, clustermembers | create, patch, delete | form, join, role, drain and release, reconciled by stormcluster |
| storm.io | cloudimages | delete | removing a VM image |

The kubevirt, snapshot and cluster kinds are also read (get, list, watch).

## What needs a signed-in identity

The console offers these, but its own role refuses them, because they are not a component's settings. Someone with
no kubernetes identity gets the apiserver's 403 in words. Anyone signed in with an identity allowed to do them, such
as the install token (#66), is unaffected:

- **Existing projects:** their members and isolation policies, and deleting them.
- **Workloads in them:** Import YAML and templates, editing or deleting any object through the generic YAML routes,
  deleting a pod.
- **Machines in them:** creating and deleting a virtual machine.
- **Secrets:** every Secret outside the keys home, including a project's Secrets tab.

## The one write this role does not bound: new projects

rustkube binds `self-provisioner` to `system:authenticated`, so anyone with a token, this ServiceAccount included,
can request a project and holds `admin` in it. No Role can take that away. So someone with no identity can still
make a new project and work inside it, as the ServiceAccount, and only there (`deploy/verify-rbac.sh` shows both).

That keeps Create → New project working on a node nobody has signed in to, but those writes are not on the list
above. To close it, empty the subjects of the `self-provisioners` ClusterRoleBinding (rustkube's README), which ends
self-service projects for every user, not only the console.

## SSH keys (#42)

The owner decided that users' keys live in a system namespace only the console writes. So:

- **`storm-users`:** stormcos ships this namespace, so the console needs no cluster-wide create on namespaces. Role
  `stormconsole-keys`: `secrets` get, list, create, update, delete.
- **`default`:** role `stormconsole-keys-migration`: `secrets` get, list, delete, for the one-time move. Drop it and
  its binding once every node has run it.

The code does not use these yet: #42 moves the keys, and until then they stay in `[vm] ssh_keys_namespace` as the
viewer.

## For stormcos

- **Subject:** ServiceAccount `kube-system/stormconsole`, whose token stormcert mints and `[kubernetes] token_file`
  reads (#33). `89-vmimages-console.yaml` binds `User stormconsole`, the client pair for vmimages, and is separate.
- **Order:** a console that today reaches the apiserver as anonymous-admin can do everything. Bind this role together
  with a way to sign in with an identity (stormcos#256), or the actions under "What needs a signed-in identity" stop
  on a stock node with no way to get them back there.

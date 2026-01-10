# Cyan Protocol flows

## Group Invite - Request Snapshot flow

```ascii
┌─────────────────────────────────────────────────────────────────────────────┐
│ FULL PROTOCOL FLOW                                                          │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│ STEP 0: GROUP INVITE (out-of-band)                                          │
│   - Host shares invite link/QR containing group_id                          │
│   - Joiner receives invite → group is created in their DB                   │
│   - This is xaero_group_invite FFI                                          │
│                                                                             │
│ STEP 1: DISCOVERY TOPIC                                                     │
│   - Both peers subscribe to: cyan/discovery/{discovery_key}                 │
│   - Bootstrap node helps initial mesh formation                             │
│                                                                             │
│ STEP 2: GROUPS EXCHANGE                                                     │
│   - Host broadcasts: GroupsExchange { groups: [TEST_GROUP] }                │
│   - Joiner broadcasts: GroupsExchange { groups: [TEST_GROUP] }              │
│   - DiscoveryActor finds shared_groups = [TEST_GROUP]                       │
│                                                                             │
│ STEP 3: PEER INTRODUCTION                                                   │
│   - Host sends: PeerIntroduction { group: TEST_GROUP, peers: [host_pk] }    │
│   - DiscoveryActor → NetworkActor: JoinPeersToTopic                         │
│                                                                             │
│ STEP 4: GROUP TOPIC                                                         │
│   - TopicActor subscribes to: cyan/group/{group_id}                         │
│   - Broadcasts RequestSnapshot                                              │
│                                                                             │
│ STEP 5: SNAPSHOT                                                            │
│   - Host responds with GroupSnapshotAvailable                               │
│   - Joiner opens direct QUIC → receives snapshot frames                     │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘



```
# Snapshot protocol test
the objective is to isolate snapshot exchange INDEPENDENT of
our actor hierarchy and other networking stuff.

## How it is supposed to flow

```ascii
┌─────────────────────────────────────────────────────────────────────────────┐
│ HOST                                    JOINER                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  1. Connect to relay                    1. Connect to relay                 │
│  2. Subscribe: cyan/discovery/{key}     2. Subscribe: cyan/discovery/{key}  │
│  3. Has TEST_GROUP with content         3. Has TEST_GROUP (empty - invited) │
│                                                                             │
│  4. Broadcast GroupsExchange ─────────────────────────────────────────────► │
│  ◄───────────────────────────────────── 5. Broadcast GroupsExchange         │
│                                                                             │
│  6. Sees shared group                   7. Sees shared group                │
│  8. Broadcast PeerIntroduction ───────────────────────────────────────────► │
│                                                                             │
│  9. Subscribe: cyan/group/{id}          10. Subscribe: cyan/group/{id}      │
│                                                                             │
│  ◄─────────────────────────────────────11. Broadcast RequestSnapshot        │
│  12. Broadcast SnapshotAvailable ─────────────────────────────────────────► │
│                                                                             │
│  13. Accept SNAPSHOT_ALPN               14. Connect SNAPSHOT_ALPN           │
│  15. Send frames ─────────────────────────────────────────────────────────► │
│                                         16. Receive frames, verify          │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘

```
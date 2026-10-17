# Finite harness finish plan and estimates

Source `6cf71ed4efb9b1f256b2fcd00e95748309fa6d0b`. These are remaining engineering-work estimates,not dates,observed throughput or a request to redo merged repairs. One engineer-day is **eight focused engineering hours**,including review and acceptance. Candidate reuse,native host access and a familiar Rust engineer are assumed;waiting and unrelated product work are excluded.

| ID  | Bounded outcome / acceptance                                                                                                             | Dependencies and existing work                                | Engineer-days | Confidence                                                   |
| --- | ---------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- | ------------- | ------------------------------------------------------------ |
| W0  | Bind owned source,installed products,daemon/socket,required profiles and independent oracle                                              | Current source/installed split;existing audit PR106           | 1-2           | Medium                                                       |
| W1  | Reconcile merged changes and open feedback into one release candidate;resolve actual current gate failures without replaying closed work | W0;merged PR95/93/98;PR97/96/100/105;issue104                 | 2-5           | Low;overlap and final candidate unsettled                    |
| W2  | Qualify exact Windows/Herdr launch,resume,resize,Enter/input,abandonment and cancellation behavior on selected installed profiles        | W1;issue45;PR95/97;upstream1812 read-only                     | 2-6           | Low;native reproduction outcome                              |
| W3  | Durable fault-safe resume and preserved session migration with restrictions,effect state,original backups and rollback                   | W0/W1;storage/session/multi-client code                       | 3-7           | Low;cross-host authority and state formats                   |
| W4  | Prove installed fork updater/storage isolation:denial,renamed executables,env overrides,rollback and Jcode preservation                  | W1;PR98 source;PR96/closed94 feedback                         | 2-5           | Medium-low;source boundary clearer than native proof         |
| W5  | Provider/tool/child fault workload preserves permission envelope and reconciles uncertain effects;no orphaned children                   | W1/W3;provider/swarm/tool components                          | 4-9           | Low;fault/retry/cancel interactions                          |
| W6  | Live manager/lane/host map plus bounded overflow and result return through both required dispatch paths                                  | W2/W3/W5;Herdr/Cursor integration and accepted manager owners | 3-6           | Low;external pool/authority boundaries                       |
| W7  | Exact owned artifact release/install/update-denial/rollback and native acceptance receipts;free local gates where hosted unavailable     | W2-W6;existing release scripts                                | 3-6           | Medium-low                                                   |
| W8  | Retained compact-terminal/accessibility/native interaction profiles pass without dropped controls or identity confusion                  | W2/W3;existing tiny-terminal work                             | 2-5           | Low;required profile inventory                               |
| W9  | Independent owned/upstream comparison and consumer/patch-carry review with real task/fault results                                       | W0/W5/W7;exact upstream/base profile                          | 3-7           | Low;first comparative result can reject advantage hypothesis |
| W10 | Close explicitly retained advanced ACP/discovery/poke/ambient/automation promises or approved replacement/sunset with consumer evidence  | W0/W5/W6/W8;PR10/25/99;accepted scope decision                | 4-12          | Low;retained feature horizon unresolved                      |

| Horizon                     | Included packages | Cumulative effort                          |
| --------------------------- | ----------------- | ------------------------------------------ |
| CVP                         | W0-W7             | 20-46 engineer-days /160-368 focused hours |
| MVP                         | CVP + W8/W9       | 25-58 engineer-days /200-464 hours         |
| Full finite mapped maturity | MVP + W10         | 29-70 engineer-days /232-560 hours         |

CVP is a sequencing slice,not a permission to waive safety,delete existing capability or stop preserving upstream/profile consumers. If W0 establishes a W8/W10 behavior as essential to the actual user,it moves into CVP and the range changes. No estimate assumes that a stronger model substitutes for permission/cancellation correctness.

## Conditional calendar scenario

Assume two dedicated implementers,independent native/assurance review available weekly,and70% effective availability:approximately7 eight-hour engineer-days/week,or56 focused hours/week. Pure capacity division gives CVP2.9-6.6 weeks,MVP3.6-8.3 and full4.1-10 weeks. Shared integration files,native fault investigation and release acceptance serialize work,so these are optimistic lower-model calculations.

An illustrative dependency-aware scenario is **CVP4-9 elapsed weeks,MVP5-12 weeks,full6-15 weeks** after a genuinely staffed start. This is not a delivery commitment or a date forecast;external waiting,new scope and major architecture replacement are excluded. Single-engineer or intermittent host access extends elapsed time;adding many concurrent agents does not divide the critical path linearly. Revise after W0/W1 and first Windows plus cross-host resume receipts.

```text
W0 -> W1 -> W2 ------------------+
 |      +-> W3 -> W5 -> W6 ------+-> W7 -> W9
 |      +-> W4 -----------------+        ^
 +-----------------------------> W8 ----+
 W0/W5/W6/W8 -> W10 (retained advanced scope)
```

## First bounded wave

1. Reuse PR106's detailed audit and existing owner work;freeze the correct source/runtime/profile identity. Record merged-versus-open candidates rather than repeating old issue states.
2. Independent assurance prepares stream disconnect,rate-limit,failed child,critical restriction,uncertain mutation and restart fixtures before acceptance. Keep forbidden repo/protected file as genuine negatives.
3. Separate writers for Windows input/liveness,session state/migration and updater/install proof;serialize shared integration paths. Heavy checks run on the stronger host with isolated sockets/state and bounded concurrency. Never kill shared Rust processes wholesale.
4. Demonstrate one real overflow task per authorized path only after identity/permission prerequisites;record manager,parent/child,host,served model,input/output and acceptance. Do not send unsolicited network messages as part of this documentation audit.

## Finish receipts

Each accepted result includes exact source/upstream-base SHA,artifact hash/version,launcher and daemon/socket identity,profile/state root,tool permission envelope,child ownership and failed-attempt history. Source/local gates,hosted CI,native fault behavior,release installation and actual task benefit remain separate acceptance states.

Migration receipt maps source session to destination identity,preserves the original,demonstrates resume with critical restrictions and states remaining laptop sessions or an explicit schedule. Overflow receipt must show work actually served elsewhere and a correct result returning;queue depth,connected socket or named pane alone is insufficient. Compare useful task completion,memory/tail latency,manual recoveries,forbidden actions and patch carry cost against the exact upstream comparator before claiming superiority.

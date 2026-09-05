# Generated guidance size

Run `python3 benchmarks/guidance-size/measure.py --base b77b4b511667b89075ec4ac89590a2ec2757f8a7`
from the repository root. The script compiles the renderer in a temporary,
dependency-free driver after removing its CLI-only clap derive. It does not
launch Packet28 or configure hooks.

`results.json` records the baseline revision, current renderer hash, compiler,
and exact UTF-8 byte lengths for all five formats with the default root. The
shared workflow removes 262 to 564 bytes per fragment while retaining runtime
capabilities and command names. Updated guidance distinguishes brief
supersession from editing earlier messages in a cached conversation.

These are output-size measurements. They do not measure model tokens, provider
cache hits, cost, latency, or instruction adherence. A guidance update changes
an installed prefix once; existing installations change only when regenerated.

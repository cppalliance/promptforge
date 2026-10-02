# shared-loopback

- This crate solely owns peer and `Host`-authority checks for loopback-bound product surfaces. Consumers call these checks instead of reimplementing them.
- `gateway_loopback_origin_allowed` and `workshop_same_origin_authority_allowed` are separately named, fail-closed predicates with distinct policies; never merge or share their policy semantics.

---
title: "Deployment Operator"
description: "A Deployment Operator is the person or automation responsible for Navigator infrastructure and rollouts."
---

The person or automation that owns Kubernetes, cloud accounts, secrets, domains, mounted deployment configuration, and
rollouts. This is distinct from an application [Role](role.md): a Person with `person.role = 'admin'` has application
authorization but does not thereby gain infrastructure access.

---
name: surreal-migrations
description: >
  Change the SurrealDB schema safely: add, retype, or remove a field in `store/src/schema/navigator.surql`, bump
  `SCHEMA_VERSION`, and ship the backfill that the applied-schema model cannot perform for you. Trigger when the user
  says "add a field", "change the schema", "write a migration", "backfill", "retype a column", or when a change edits
  `navigator.surql` or a row struct that reads it. Covers why a green test suite proves nothing about staging and
  production.
---

# Surreal migrations

Read the module docs in [`store/src/schema/mod.rs`](../../../store/src/schema/mod.rs) and the "Production database"
section of [`docs/cloud-operations.md`](../../../docs/cloud-operations.md) before acting. The schema is a statement of
the present, not a chain of steps, and that choice is what makes a data change your job rather than the engine's.

## What applying the schema does and does not do

[`navigator.surql`](../../../store/src/schema/navigator.surql) is one idempotent file describing the tables and fields
that should exist. `store::schema::apply` runs it whole on every boot and in every test that opens an embedded engine,
then records `SCHEMA_VERSION`. Bump that constant in the same change that edits the file, or a database prepared by
another build reports as in sync while disagreeing.

Applying converges *definitions*. It does not touch a single existing row:

- `DEFINE TABLE IF NOT EXISTS` deliberately leaves an existing table's rows alone.
- `DEFAULT` on a `DEFINE FIELD` is a **write-time** default. It supplies a value for rows written after the definition
  lands. It does not reach back into rows written before it, which keep no value for that field at all.

So adding a required field is two changes, not one: the definition, and a backfill for the rows that predate it.
Backfills stay explicit one-shot jobs.

## The failure this prevents

A row struct that reads the new field as a bare `bool` cannot deserialize a row written before the field existed. The
engine reports no value, and the read fails with `Expected bool, got none`. When that read sits on a boot path, the
binary crash-loops against any database with history, and rolling back is the only way out.

`26.9.3` shipped exactly this. `email_confirmed` was defined on `person` with `DEFAULT false` and read as
`email_confirmed: bool` in `PersonRow` ([`store/src/persons.rs`](../../../store/src/persons.rs)), with no backfill.
Fresh databases were fine. Staging and production, which hold person rows older than the field, crash-looped
`navigator-web` in fixture seeding on boot.

**A green workspace proves nothing here.** Every test opens a fresh embedded engine, so every row it reads was written
by the current schema and carries every field. The only databases that hold pre-field rows are staging and production,
and nothing in CI resembles them.

## Adding or retyping a field

1. **Define it.** Edit `navigator.surql`. Prefer `option<T>` for anything a historical row may lack. Reach for a
   non-optional `T` with `DEFAULT` only when you are also shipping step 3.
2. **Bump `SCHEMA_VERSION`** in [`store/src/schema/mod.rs`](../../../store/src/schema/mod.rs), same change.
3. **Make the read tolerate a missing value, or backfill before the read ships.** Typing the row struct field
   `Option<T>` and collapsing it in the `into_*` conversion (`.unwrap_or_default()`) is the cheaper half and survives a
   backfill that has not run yet. A backfill alone leaves the crash live for the window between deploy and job.
4. **Cover the historical row.** Write a test that creates a record without the new field (raw `CREATE` or `UPDATE
   ... UNSET`), then reads it through the same accessor production uses. Without this the suite only ever sees rows the
   current schema wrote, which is the hole that let `26.9.3` through.
5. **Write the backfill.** Idempotent and guarded on the old value, so re-running it is safe and it cannot overwrite a
   real value:

   ```surql
   UPDATE person SET email_confirmed = false WHERE email_confirmed IS NONE;
   ```

6. **Run it per row, staging first.** Follow `docs/cloud-operations.md` § "Production database": write the exact
   SurrealQL to a timestamped file under `/tmp/navigator-prod-sql/`, show the user the path and contents, wait for
   approval of that exact statement, then verify the row count afterwards. Read-only `SELECT`s need no ceremony; every
   `UPDATE`, `DELETE`, and `DEFINE` does.

## Removing a field

Stop reading it first, in a released build, then drop the definition in a later change. A definition removed while a
deployed binary still selects the column turns every read into a failure with no rollback path except redeploying the
old tag.

## Checklist

- `navigator.surql` edited and `SCHEMA_VERSION` bumped in the same commit.
- Every new non-optional field has either a tolerant reader or a backfill that ships first.
- A test covers a row written without the field.
- The backfill is idempotent, guarded, and recorded where the operator running it will find it.
- Staging ran the backfill and served the new binary before production saw either.

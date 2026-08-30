# Security runbook

Operational notes for moderators and maintainers. This file is for the people who
run the bot, not for contributors setting up a dev environment (that is `README.md`).

## Undoing a verification link (`/unlink`)

The bot links a Discord account to a student id in the `dsec_discord_members` table
when someone verifies. Because verification only checks a **name + student id** pair —
both semi-public — a person can verify as someone else. When that happens, undo it.

### Preferred: the `/unlink` command

Run `/unlink @member` in the server. It requires the **Manage Roles** permission and:

1. deletes the member's `dsec_discord_members` row, **then**
2. removes the verified role from them,
3. replies to you privately (ephemeral) with what it did, and
4. writes a log entry to the logs channel naming you as the moderator who ran it.

It does the delete first and the role removal second **on purpose**: the reverse order
can leave someone un-roled but still linked, which permanently breaks `/member_info`
for them. If the role removal half fails, the reply and the log both say so — finish it
by hand (next section) and the row is already gone.

### Manual removal (when `/unlink` cannot be used)

You need the Supabase project credentials for this. **Who holds them:** the club
committee — ask in the committee Discord. (Historically the bot maintainer; TODO: name
the current holder here.) Do **not** paste real credentials into a chat or a ticket.

Find the link row(s) for a student id:

```sql
select * from dsec_discord_members where student_id = 's123456789';
```

Delete a specific link by hand:

```sql
delete from dsec_discord_members where discord_id = '<discord user id>';
```

**Deleting the row does NOT revoke the Discord role.** Removing the row and stripping
the verified role are two separate actions — that is exactly why `/unlink` does both.
After a manual delete, also remove the verified role from the member in Discord (Server
Settings → Members, or right-click the member → Roles), or they keep their access with
no link.

## Owner-only database step (SEC-19) — MERGE / DEPLOY GATE

**This is a deploy gate, not optional.** `dsec_discord_members.student_id` MUST have a
`UNIQUE` constraint so one student id cannot be claimed by two Discord accounts. Run
the duplicate sweep and add the constraint (below) as part of shipping this change.

The application also refuses the second claimant and catches a concurrent-insert unique
violation (SQLSTATE 23505), converting it to a generic refusal — but the check-then-
insert has an inherent race, so the database constraint is what actually guarantees
uniqueness. Until the constraint exists, two accounts verifying the same unused id at
the exact same moment can both succeed. The constraint is **not** applied by any
migration in this repo — it touches live data and must be run by a maintainer.

Sweep for existing duplicates first; the DDL fails if any exist:

```sql
select student_id, count(*) from dsec_discord_members
group by student_id having count(*) > 1;
```

Resolve any duplicates by hand (decide which Discord account keeps each link), then:

```sql
alter table dsec_discord_members add constraint dsec_discord_members_student_id_key unique (student_id);
```

There is no staging Supabase project — do the sweep and the `ALTER` with a second
maintainer watching. Expect it to start rejecting inserts that used to succeed.

> The real fix for the weak identity check is a possession proof — email a one-time
> code to the roster address (dsec-app already owns OTP machinery; the bot would call
> dsec-api). That is feature-sized work tracked under SEC-19, not covered here.

# prtop glossary

## Change request

An open proposal to merge one branch into another on a forge. GitHub calls it a pull request and GitLab calls it a merge request. `ChangeRequest` is the canonical prtop term.

## Forge account

A configured forge identity, uniquely named in configuration. It has a forge type and host. Multiple accounts may point at the same host.

## Project

A named repository registration. A project may have a local or remote working directory, but it always belongs to one forge account.

## Pipeline

The provider's execution grouping for CI work. A pipeline contains jobs.

## Job

One executable CI unit in a pipeline. A job has a provider-stable ID and may have more than one attempt. Retrying does not overwrite earlier attempts.

## Log chunk

An increment of a job's textual output. prtop bounds retained log lines and may discard the oldest lines with an explicit marker.

## Review

An explicit response by a reviewer. Requested, pending, approved, changes requested, commented,
and dismissed are normalized states. Provider approval rules remain provider metadata.

## Comment

A general change-request discussion entry. A comment has a stable provider ID, author, body,
creation and optional edit times, and provider permissions for editing or deletion.

## Stack

An ordered parent-child branch relationship, optionally connected to change requests. It is not inferred merely because two change requests share a repository.

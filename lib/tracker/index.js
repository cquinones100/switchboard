// The ticket tracker, Linear or Jira, whichever the `tracker` setting names:
// one per machine. Both answer the same questions in the same shape, Linear's
// (see linear.js and jira.js), so nothing that asks needs to know which it
// is; what only one of them can answer (Linear's inbox) is missing from the
// other, and the caller checks for it.
//
//   tracker()        the provider, or undefined with the setting off
//   idFrom(text)     a ticket ID from an ID, a Linear or Jira URL, or a branch

const { currentSettings } = require("../settings");
const linear = require("./linear");
const jira = require("./jira");

const PROVIDERS = { linear, jira };

const trackerKey = () => currentSettings().find((o) => o.key === "tracker")?.value ?? "linear";

const tracker = (key = trackerKey()) => PROVIDERS[key];

// Any team or project key: Linear's (ABC-123) and Jira's (ABC-123, or
// AB2-123) look the same.
const ID = /^[A-Za-z][A-Za-z0-9]*-\d+$/;

// A ticket ID, upper-cased, from a bare ID, a Linear issue URL
// (linear.app/<workspace>/issue/ABC-123/...) or a Jira one (/browse/ABC-123,
// or a board's ?selectedIssue=ABC-123), else undefined.
const idFrom = (text) => {
  const input = String(text ?? "").trim();
  const found =
    /\/issue\/([A-Za-z][A-Za-z0-9]*-\d+)/.exec(input)?.[1] ??
    /\/browse\/([A-Za-z][A-Za-z0-9]*-\d+)/.exec(input)?.[1] ??
    /[?&]selectedIssue=([A-Za-z][A-Za-z0-9]*-\d+)/.exec(input)?.[1] ??
    (ID.test(input) ? input : undefined);
  return found?.toUpperCase();
};

module.exports = { PROVIDERS, trackerKey, tracker, idFrom, ID };

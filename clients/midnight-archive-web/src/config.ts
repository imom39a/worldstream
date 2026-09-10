export const MIDNIGHT_ARCHIVE_PACK_ID = "worldstream.midnight-archive" as const;
export const MIDNIGHT_ARCHIVE_PACK_VERSION = "0.1.0" as const;

export const MIDNIGHT_ARCHIVE_PACK_REVISION =
  "blake3:aea45a1c056c4a7da744be33d38df672499062fd2548302fae43adc49b833b07" as const;

export const MIDNIGHT_ARCHIVE_CLIENT_PATH = "/midnight-archive-v13/" as const;

/** Fixed by the reviewed hosted v13 integration, never read from Pack state. */
export const MIDNIGHT_ARCHIVE_HOSTED_EXHIBITION_TERMS = Object.freeze({
  includedAtNoCharge: true as const,
  maximumCallsPerAgent: 10,
  maximumInputTokensPerAgent: 120_000,
  maximumOutputTokensPerAgent: 10_000,
  callTimeoutSeconds: 60,
});

export type MidnightArchiveHostedExhibitionTerms =
  typeof MIDNIGHT_ARCHIVE_HOSTED_EXHIBITION_TERMS;

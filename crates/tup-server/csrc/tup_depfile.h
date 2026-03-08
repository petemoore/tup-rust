/*
 * tup_depfile.h - Shared header for the dependency file protocol.
 *
 * This defines the wire format used between the LD_PRELOAD library
 * and the tup process server.
 */

#ifndef TUP_DEPFILE_H
#define TUP_DEPFILE_H

/* Environment variable pointing to the depfile path */
#define TUP_DEPFILE "TUP_DEPFILE"

/* Access event types — must match tup_types::AccessType */
enum access_type {
	ACCESS_READ   = 0,
	ACCESS_WRITE  = 1,
	ACCESS_RENAME = 2,
	ACCESS_UNLINK = 3,
	ACCESS_VAR    = 4,
};

/*
 * Wire format for a file access event.
 *
 * Written to the depfile as:
 *   struct access_event (12 bytes)
 *   path1 (len bytes, including NUL)
 *   path2 (len2 bytes, including NUL, if len2 > 0)
 */
struct access_event {
	int at;    /* enum access_type */
	int len;   /* length of path1 including NUL */
	int len2;  /* length of path2 including NUL (0 if none) */
};

#endif

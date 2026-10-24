/* SPDX-License-Identifier: LGPL-3.0-or-later */
/*
 * ops_pointcloud.h - C interface of the point-cloud core of Open Pointcloud Studio.
 * ABI 1.0. See docs/library.md for ownership, errors, threads and versioning.
 *
 * Every function that returns an OpsStatus clears the message of the calling
 * thread first and sets it when it fails; ops_last_error() returns it. That
 * text belongs to the library and stays valid until the next call of the
 * library on the same thread; it is never freed by the caller. Out
 * pointers are NULL (or 0) after a failure. Clouds and indexes are freed only
 * with their own _free function, strings returned through char** only with
 * ops_string_free. Callbacks run on the calling thread and must not unwind
 * (no C++ exception, no longjmp past the library).
 */
#ifndef OPS_POINTCLOUD_H
#define OPS_POINTCLOUD_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Define OPS_STATIC before including this header when linking the static
   archive on Windows. */
#if defined(_WIN32) && !defined(OPS_STATIC)
#  define OPS_API __declspec(dllimport)
#else
#  define OPS_API
#endif

#define OPS_ABI_MAJOR 1
#define OPS_ABI_MINOR 0

typedef int32_t OpsStatus;
#define OPS_OK               0
#define OPS_ERROR_ARGUMENT   1  /* a null pointer, an unknown value, a struct_size too small, a path not in UTF-8 */
#define OPS_ERROR_IO         2  /* the file system refused */
#define OPS_ERROR_FORMAT     3  /* the extension names no supported format */
#define OPS_ERROR_DATA       4  /* the file is not a readable point cloud, or the source changed */
#define OPS_ERROR_CANCELLED  5  /* a callback asked to stop */
#define OPS_ERROR_INTERNAL   6  /* an internal error was caught; ops_last_error says where */
#define OPS_ERROR_LIMIT      7  /* the result would hold more than max_points points */

typedef int32_t OpsFormat;
#define OPS_FORMAT_BY_EXTENSION 0
#define OPS_FORMAT_LAS        1
#define OPS_FORMAT_LAZ        2
#define OPS_FORMAT_E57        3
#define OPS_FORMAT_PLY        4    /* binary little-endian */
#define OPS_FORMAT_PLY_ASCII  5
#define OPS_FORMAT_XYZ        6
#define OPS_FORMAT_PTS        7
#define OPS_FORMAT_CSV        8

#define OPS_POINT_RGB            1u
#define OPS_POINT_INTENSITY      2u
#define OPS_POINT_CLASSIFICATION 4u

typedef struct OpsCloud OpsCloud;   /* an opened source file */
typedef struct OpsIndex OpsIndex;   /* an octree index on disk */

/* Fixed for every ABI 1.x: a new layout comes with new functions. */
typedef struct OpsPoint {           /* 40 bytes */
    double   x, y, z;
    uint16_t intensity;             /* valid when flags & OPS_POINT_INTENSITY */
    uint8_t  r, g, b;               /* valid when flags & OPS_POINT_RGB */
    uint8_t  classification;        /* valid when flags & OPS_POINT_CLASSIFICATION */
    uint8_t  flags;
    uint8_t  reserved;              /* zero */
    uint64_t ordinal;               /* position of the point in its source, from 0 */
} OpsPoint;

/* Fixed for every ABI 1.x. */
typedef struct OpsBox {             /* 56 bytes */
    double min[3];
    double max[3];
    double rotation_degrees;        /* counter-clockwise seen from above, about the vertical through the centre */
} OpsBox;

/* Structs with struct_size may grow in a minor version: fields are only appended. */
typedef struct OpsCloudInfo {       /* 72 bytes in ABI 1.0 */
    uint32_t struct_size;           /* in: sizeof(OpsCloudInfo) */
    uint32_t station_count;
    uint64_t point_count;
    double   min[3];
    double   max[3];
    uint32_t station_photo_count;
    uint8_t  has_rgb;
    uint8_t  has_intensity;
    uint8_t  has_classification;
    uint8_t  reserved;
} OpsCloudInfo;

typedef struct OpsExportOptions {   /* 24 bytes in ABI 1.0 on 64-bit systems */
    uint32_t      struct_size;      /* in: sizeof(OpsExportOptions) */
    OpsFormat     format;
    const OpsBox *box;              /* NULL: every point */
    uint64_t      max_points;       /* 0: no limit; more gives OPS_ERROR_LIMIT and no file */
} OpsExportOptions;

/* Called during long work; fraction from 0 to 1, or -1 when unknown.
   Return 0 to go on, anything else to stop with OPS_ERROR_CANCELLED. */
typedef int32_t (*OpsProgressFn)(double fraction, void *user_data);

typedef struct OpsIndexOptions {    /* 32 bytes in ABI 1.0 on 64-bit systems */
    uint32_t      struct_size;      /* in: sizeof(OpsIndexOptions) */
    int32_t       cached;           /* 1: in the cache folder of the application (needs ops_set_disk_caches(1)) */
    const char   *scratch_dir;      /* UTF-8 folder for an uncached index; NULL: the system temp folder */
    OpsProgressFn progress;         /* may be NULL */
    void         *user_data;
} OpsIndexOptions;

typedef struct OpsIndexInfo {       /* 32 bytes */
    uint32_t struct_size;           /* in: sizeof(OpsIndexInfo) */
    uint32_t depth;
    uint64_t point_count;
    uint64_t node_count;
    uint64_t leaf_count;
} OpsIndexInfo;

/* Receives a batch of points on the calling thread; the buffer belongs to the
   library and is valid only during the call. Return 0 to go on, anything
   else to stop with OPS_ERROR_CANCELLED. */
typedef int32_t (*OpsPointsFn)(const OpsPoint *points, size_t count, void *user_data);

OPS_API uint32_t    ops_abi_version(void);        /* (OPS_ABI_MAJOR << 16) | OPS_ABI_MINOR of the loaded library */
OPS_API const char *ops_version(void);            /* product version, e.g. "2026.44.0"; static */
OPS_API const char *ops_last_error(void);         /* message of the last failed call on this thread, or NULL */
OPS_API void        ops_string_free(char *text);  /* frees a string returned through char**; NULL is ignored */
OPS_API void        ops_set_disk_caches(int32_t enabled); /* process-wide; off unless set to 1 */

OPS_API OpsStatus ops_cloud_open(const char *path, OpsCloud **cloud);
OPS_API void      ops_cloud_free(OpsCloud *cloud);
OPS_API OpsStatus ops_cloud_info(const OpsCloud *cloud, OpsCloudInfo *info);
OPS_API OpsStatus ops_cloud_info_json(const OpsCloud *cloud, char **json);
OPS_API OpsStatus ops_cloud_read(const OpsCloud *cloud, const OpsBox *box, size_t batch_points,
                                 OpsPointsFn callback, void *user_data);  /* box NULL: all; batch 0: 65536, at most 1048576 */
OPS_API OpsStatus ops_cloud_export(const OpsCloud *cloud, const char *destination,
                                   const OpsExportOptions *options, uint64_t *written); /* options NULL: by extension, all; written may be NULL */

OPS_API OpsStatus ops_index_build(const OpsCloud *cloud, const OpsIndexOptions *options, OpsIndex **index); /* options NULL: uncached, temp folder */
OPS_API void      ops_index_free(OpsIndex *index);
OPS_API OpsStatus ops_index_info(const OpsIndex *index, OpsIndexInfo *info);
OPS_API OpsStatus ops_index_read(const OpsIndex *index, const OpsBox *box, size_t batch_points,
                                 OpsPointsFn callback, void *user_data);

#ifdef __cplusplus
}
#endif
#endif

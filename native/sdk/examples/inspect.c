/* SPDX-License-Identifier: LGPL-3.0-or-later */
/*
 * inspect.c - the C library of Open Pointcloud Studio from a C program.
 *
 *   inspect [FOLDER]
 *
 * Writes a grid of 1000 points to FOLDER/grid.xyz (FOLDER is "." by
 * default), opens it, prints what it holds as JSON, reads its points,
 * exports half of them to FOLDER/grid.laz, opens that, builds an index of it
 * in FOLDER and reads the same half through the index. It prints one line
 * more and returns 0 when every result is what it should be; otherwise it
 * prints the message of the library and returns 1.
 *
 * Build lines are in README.md beside this folder and in docs/library.md.
 */
/* fopen as the C standard has it, without the warning of the Windows C
   compiler about it. */
#ifndef _CRT_SECURE_NO_WARNINGS
#define _CRT_SECURE_NO_WARNINGS
#endif

#include <inttypes.h>
#include <stdio.h>
#include <string.h>

#include "ops_pointcloud.h"

/* The layouts that ABI 1.x promises. */
_Static_assert(sizeof(OpsPoint) == 40, "OpsPoint is 40 bytes");
_Static_assert(sizeof(OpsBox) == 56, "OpsBox is 56 bytes");
_Static_assert(sizeof(OpsCloudInfo) == 72, "OpsCloudInfo is 72 bytes");
_Static_assert(sizeof(OpsIndexInfo) == 32, "OpsIndexInfo is 32 bytes");
_Static_assert(sizeof(void *) != 8 || sizeof(OpsExportOptions) == 24, "OpsExportOptions is 24 bytes");
_Static_assert(sizeof(void *) != 8 || sizeof(OpsIndexOptions) == 32, "OpsIndexOptions is 32 bytes");

#define SIDE 10
#define POINTS (SIDE * SIDE * SIDE)

/* Whether the library of version `abi` serves a program built for `major`
   that uses the functions of minor version `minor`. */
static int abi_fits(uint32_t abi, uint32_t major, uint32_t minor)
{
    return abi >> 16 == major && (abi & 0xffffu) >= minor;
}

/* What a read collects. */
typedef struct Tally {
    uint64_t count;
    double min[3];
    double max[3];
} Tally;

static int32_t tally_points(const OpsPoint *points, size_t count, void *user_data)
{
    Tally *tally = (Tally *)user_data;
    size_t i;
    for (i = 0; i < count; i++) {
        double xyz[3];
        int axis;
        xyz[0] = points[i].x;
        xyz[1] = points[i].y;
        xyz[2] = points[i].z;
        for (axis = 0; axis < 3; axis++) {
            if (tally->count == 0 || xyz[axis] < tally->min[axis]) tally->min[axis] = xyz[axis];
            if (tally->count == 0 || xyz[axis] > tally->max[axis]) tally->max[axis] = xyz[axis];
        }
        tally->count++;
    }
    return 0;
}

static int32_t count_progress(double fraction, void *user_data)
{
    int *reports = (int *)user_data;
    (void)fraction;
    (*reports)++;
    return 0;
}

/* Prints why the last call failed and returns 1. */
static int failed(const char *what, OpsStatus status)
{
    const char *message = ops_last_error();
    fprintf(stderr, "inspect: %s failed with status %d: %s\n", what, (int)status,
            message ? message : "(no message)");
    return 1;
}

static int wrong(const char *what)
{
    fprintf(stderr, "inspect: %s\n", what);
    return 1;
}

static int write_grid(const char *path)
{
    FILE *file = fopen(path, "w");
    int i, j, k;
    if (!file) return wrong("cannot write the grid");
    for (i = 0; i < SIDE; i++)
        for (j = 0; j < SIDE; j++)
            for (k = 0; k < SIDE; k++)
                fprintf(file, "%.3f %.3f %.3f\n", i * 0.1, j * 0.1, k * 0.1);
    return fclose(file) == 0 ? 0 : wrong("cannot write the grid");
}

int main(int argc, char **argv)
{
    const char *folder = argc > 1 ? argv[1] : ".";
    char grid_path[1024], laz_path[1024];
    uint32_t abi = ops_abi_version();
    OpsCloud *cloud = NULL, *half = NULL;
    OpsIndex *index = NULL;
    OpsCloudInfo info;
    OpsIndexInfo index_info;
    OpsExportOptions export_options;
    OpsIndexOptions index_options;
    /* The lower half of the grid along x. */
    OpsBox lower = {{-1.0, -1.0, -1.0}, {0.45, 2.0, 2.0}, 0.0};
    Tally all = {0, {0}, {0}}, through_index = {0, {0}, {0}};
    char *json = NULL;
    uint64_t written = 0;
    int reports = 0;
    OpsStatus status;
    int result = 1;

    /* The library this program runs with must have the major version it was
       built for, and at least the minor version whose functions it uses. */
    if (!abi_fits(abi, OPS_ABI_MAJOR, OPS_ABI_MINOR))
        return wrong("the library has another ABI version");

    if (snprintf(grid_path, sizeof grid_path, "%s/grid.xyz", folder) >= (int)sizeof grid_path
        || snprintf(laz_path, sizeof laz_path, "%s/grid.laz", folder) >= (int)sizeof laz_path)
        return wrong("the folder name is too long");
    if (write_grid(grid_path) != 0) return 1;

    status = ops_cloud_open(grid_path, &cloud);
    if (status != OPS_OK) return failed("ops_cloud_open", status);

    memset(&info, 0, sizeof info);
    info.struct_size = (uint32_t)sizeof info;
    status = ops_cloud_info(cloud, &info);
    if (status != OPS_OK) { failed("ops_cloud_info", status); goto done; }
    if (info.point_count != POINTS) { wrong("the grid does not hold 1000 points"); goto done; }

    status = ops_cloud_info_json(cloud, &json);
    if (status != OPS_OK) { failed("ops_cloud_info_json", status); goto done; }
    printf("%s\n", json);
    ops_string_free(json);

    status = ops_cloud_read(cloud, NULL, 100, tally_points, &all);
    if (status != OPS_OK) { failed("ops_cloud_read", status); goto done; }
    if (all.count != info.point_count
        || memcmp(all.min, info.min, sizeof all.min) != 0
        || memcmp(all.max, info.max, sizeof all.max) != 0) {
        wrong("the points read are not those the cloud holds");
        goto done;
    }

    memset(&export_options, 0, sizeof export_options);
    export_options.struct_size = (uint32_t)sizeof export_options;
    export_options.format = OPS_FORMAT_BY_EXTENSION;
    export_options.box = &lower;
    export_options.max_points = 10;
    status = ops_cloud_export(cloud, laz_path, &export_options, &written);
    if (status != OPS_ERROR_LIMIT) { wrong("max_points did not refuse the export"); goto done; }
    export_options.max_points = 0;
    status = ops_cloud_export(cloud, laz_path, &export_options, &written);
    if (status != OPS_OK) { failed("ops_cloud_export", status); goto done; }
    if (written != POINTS / 2) { wrong("the export did not write half of the grid"); goto done; }

    status = ops_cloud_open(laz_path, &half);
    if (status != OPS_OK) { failed("ops_cloud_open of the export", status); goto done; }

    memset(&index_options, 0, sizeof index_options);
    index_options.struct_size = (uint32_t)sizeof index_options;
    index_options.cached = 0;
    index_options.scratch_dir = folder;
    index_options.progress = count_progress;
    index_options.user_data = &reports;
    status = ops_index_build(half, &index_options, &index);
    if (status != OPS_OK) { failed("ops_index_build", status); goto done; }
    if (reports == 0) { wrong("the index build reported no progress"); goto done; }

    memset(&index_info, 0, sizeof index_info);
    index_info.struct_size = (uint32_t)sizeof index_info;
    status = ops_index_info(index, &index_info);
    if (status != OPS_OK) { failed("ops_index_info", status); goto done; }

    status = ops_index_read(index, &lower, 0, tally_points, &through_index);
    if (status != OPS_OK) { failed("ops_index_read", status); goto done; }
    if (through_index.count != written || index_info.point_count != written) {
        wrong("the index does not hold the exported half");
        goto done;
    }

    printf("ops_pointcloud %s, ABI %" PRIu32 ".%" PRIu32 ": %" PRIu64 " points read, %" PRIu64
           " exported, %" PRIu64 " read through an index of %" PRIu64 " nodes\n",
           ops_version(), abi >> 16, abi & 0xffffu, all.count, written, through_index.count,
           index_info.node_count);
    result = 0;

done:
    ops_index_free(index);
    ops_cloud_free(half);
    ops_cloud_free(cloud);
    return result;
}

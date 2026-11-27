# Measure the distance from a PCD, LAS or LAZ scan to an exported Studio DXF

This independent check reads the triangles stored in a Studio mesh or detected-face DXF. It measures the shortest distance from each finite scan point to any triangle and prints the sample count, triangle count, mean, nearest-rank 95th percentile and maximum as JSON. The drawing and scan must share an origin, axes and units. The validator keeps the mesh in memory. It reads LAS and LAZ in batches of up to 250,000 points and applies the scale and offset of each axis. The PCD reader loads the scan in memory. The exact percentile calculation uses eight bytes of temporary disk space per finite point.

From the repository root, install the validation dependencies in a Python virtual environment and run the script against an exported file.

```sh
python -m pip install 'ezdxf==1.4.5' 'numpy==2.5.5' 'point-cloud-utils==0.34.0' 'pypcd4==1.5.1'
python native/validation/mesh-deviation/dxf_reference.py room_scan1.pcd terrain.dxf

# For LAS and compressed LAZ inputs
python -m pip install 'laspy[lazrs]==2.7.0'
python native/validation/mesh-deviation/dxf_reference.py survey.laz terrain.dxf
```

The reader accepts triangular model-space MESH entities without subdivision and triangular POLYLINE polyface meshes. LINE entities represent cylinder axes and are skipped. Other entity types and non-triangular faces raise an error.

## Room scan

The [public PCL room scan](https://github.com/PointCloudLibrary/data/blob/084fa0e260580cc765ded83de4ec3f495f1d6f1e/tutorials/room_scan1.pcd) has 112,586 points and SHA-256 `52c373a67d8beaa318b5e8c024f06e219f14acc1db28fa7333ff5dc73840428b`. Generate the inputs with Studio and run the validator once per drawing.

```sh
open-pointcloud-studio --mesh room_scan1.pcd terrain.obj
open-pointcloud-studio --surface room_scan1.pcd surface.obj --max-vertices 5000
open-pointcloud-studio --closed-mesh room_scan1.pcd closed.obj --voxel 0.20
for mesh in terrain surface closed; do
    open-pointcloud-studio --mesh-export "$mesh.obj" "$mesh.dxf"
done
open-pointcloud-studio --faces room_scan1.pcd planes.dxf --cylinders off
for mesh in terrain surface closed planes; do
    python native/validation/mesh-deviation/dxf_reference.py room_scan1.pcd "$mesh.dxf"
done
```

| DXF export | Triangles | Mean (m) | P95 (m) | Maximum (m) |
| --- | ---: | ---: | ---: | ---: |
| Terrain | 10,021 | 0.091599 | 0.372967 | 3.050517 |
| Surface | 13,363 | 0.022141 | 0.058458 | 5.504753 |
| Closed mesh | 15,783 | 0.020315 | 0.078777 | 2.616754 |
| 14 detected planes | 215 | 0.601679 | 1.785007 | 12.453226 |

For the cross-check, export the planes a second time with the same arguments and `planes.obj` as the output. Run `open-pointcloud-studio --mesh-deviation room_scan1.pcd FILE.obj` for each of the four OBJ files using the [Linux build of PR #86](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/actions/runs/45808338284). Across the twelve statistics the largest absolute difference observed was below 7 × 10⁻¹⁵ m. The plane row includes every measured point in the room. Studio assigned 60,088 points to detected planes and reports their per-plane fit separately.

## AHN6 terrain

The survey crop spans the boundary between two public 2025 AHN6 tiles in RD New + NAP (EPSG:7415). Download the [southern tile](https://fsn1.your-objectstorage.com/hwh-ahn/AHN6/01_LAZ/AHN6_2025_C_208000_474000.LAZ) and [northern tile](https://fsn1.your-objectstorage.com/hwh-ahn/AHN6/01_LAZ/AHN6_2025_C_208000_475000.LAZ).

```sh
box=208400,474950,-100,208500,475050,100
open-pointcloud-studio --section AHN6_2025_C_208000_474000.LAZ "$box" south.laz
open-pointcloud-studio --section AHN6_2025_C_208000_475000.LAZ "$box" north.laz
open-pointcloud-studio --merge survey.laz south.laz north.laz
open-pointcloud-studio --mesh survey.laz terrain.obj
open-pointcloud-studio --mesh-export terrain.obj terrain.dxf
python native/validation/mesh-deviation/dxf_reference.py survey.laz terrain.dxf
open-pointcloud-studio --mesh-deviation survey.laz terrain.obj
```

The DXF contained 194,895 triangles. Across all 313,232 survey points, including above-ground returns, the mean distance was 0.358270 m, P95 was 2.323809 m and the maximum was 11.425575 m. Decompressing the same records to LAS gave identical statistics. Each statistic agreed with Studio’s OBJ calculation within 0.000000000012 m. The crop and merge were verified with [PR #89](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/pull/89).

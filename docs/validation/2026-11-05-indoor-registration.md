# Indoor static/SLAM registration — 5 November 2026

This completes the remaining measured indoor workflow check of [#44](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/issues/44). It tests the existing native 2026.45.1 implementation; no registration algorithm was changed for this test.

## Public inputs and preparation

The [Oxford Spires dataset](https://dynamic.robots.ox.ac.uk/datasets/oxford-spires/) labels `2024-03-18-christ-church-03` as an indoor sequence. Its SLAM frames were collected with a handheld Hesai QT64/VILENS system; the independent reference was measured with a static Leica RTC360. Use the authors’ [Hugging Face distribution](https://huggingface.co/datasets/ori-drs/oxford_spires_dataset). The dataset is CC BY-NC-SA 4.0 and intended for non-commercial academic use; the point clouds are not redistributed here. Credit: Tao et al., *The Oxford Spires Dataset* (2025).

Download these paths under one data directory:

- `ground_truth_map/christ-church/merged-cloud-1cm.pcd` → `tls-1cm.pcd`
- `sequences/2024-03-18-christ-church-03/processed/vilens-slam/undist-clouds.zip` → `slam-clouds.zip`
- `sequences/2024-03-18-christ-church-03/processed/trajectory/gt-tum.txt` → `gt-tum.txt`
- `sequences/2024-03-18-christ-church-03/processed/trajectory/vilens-slam-tum.txt` → `vilens-slam-tum.txt`

| File | SHA-256 |
| --- | --- |
| `tls-1cm.pcd` | `0da1757d9bec20c79fa6f0698956ba6a5be988fb5b2f6fc51bcd2ada413c2f0b` |
| `slam-clouds.zip` | `0ce32964eb568d84584501701c3c0321969179f6a489ff70683b0e3dc9844bd9` |

The preparation script aligns the published SLAM and reference trajectories to obtain a coarse transform independently of OPS. It takes 30 original frames (indices 40:100:2), applies their VIEWPOINT poses, retains ranges 0.7–8 m and deterministically thins to a 3 cm grid. This yields 628,477 mobile points. It streams the 113,996,107-point TLS map through memory mapping and selects the mobile overlap bounds with a 0.3 m margin: 13,145,511 reference points. The GUI uses every tenth reference point (1,314,552); the independent distance calculation uses **all** 13,145,511 overlap reference points.

## Environment and reproduction

Linux x86-64, AMD Ryzen 7 3700U, 8 logical CPUs, 14,254,788 KiB RAM. Existing native debug development binary reporting 2026.45.1; its SHA-256 is recorded in the [raw result](../../native/validation/indoor-registration/result-2026-11-05.json). Python validation dependencies: NumPy 2.5.4, SciPy 1.18.1 and pye57 0.4.19 (independent C++ libE57 reader). They are validation dependencies, not application dependencies.

```bash
python3 native/validation/indoor-registration/prepare.py "$DATA_DIRECTORY"
# Start a separate test window with an empty config, not a user window.
XDG_CONFIG_HOME="$TEST_CONFIG_DIRECTORY" open-pointcloud-studio --api-port 42167
python3 native/validation/indoor-registration/validate.py "$DATA_DIRECTORY" \
  --config-directory "$TEST_CONFIG_DIRECTORY" --port 42167
```

The validation command expects an empty test window. It opens the two PLY inputs, sets the independently computed coarse matrix, runs point-to-plane ICP through the product API and exports a registered E57. It then checks counts and world coordinates with libE57. The script reads the test window’s token from its discovery file and does not publish it.

## Measured results

| Independent nearest-TLS-point metric | Before ICP | After ICP |
| --- | --- | --- |
| Mean | 45.110 mm | 43.868 mm |
| Median | 23.515 mm | 21.556 mm |
| 95th percentile | 138.760 mm | 139.145 mm |
| Points within 50 mm | 80.400% | 81.531% |

ICP converged in 17 iterations with 40,769 matches and took 1.21 s on this run. The application’s sampled RMS/p95 were 74.112/163.930 mm; these use its bounded ICP sample and are different measurements from the independent full-mobile-cloud statistics above. The 95th percentile did **not** improve, even though mean, median and coverage did. This is measured sensor data with partial overlap and SLAM drift, not an exact synthetic room or a scanner accuracy certification.

The registered E57 contains exactly **1,314,552 + 628,477 = 1,943,029 points**. Independent application of the saved matrix agrees with the exported mobile coordinates to a maximum of **8.14e-15 m**. This numerical round-trip check is not physical survey accuracy.

The source PCD files do not contain E57 Images2D. This closes the indoor registration validation gap of #44; it does not close the vendor panorama-export check in #42. The earlier point-pair, matrix reuse, mixed-format merge, cancellation, API and MCP checks remain recorded in [2026.45.1](2026.45.1.md).

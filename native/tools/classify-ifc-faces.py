#!/usr/bin/env python3
"""List or classify reviewed planar faces in a Studio IFC4 export."""
import argparse
from pathlib import Path
from tempfile import TemporaryDirectory


def classify(args, model, reassign_class):
    if model.schema != "IFC4":
        raise ValueError("Expected an IFC4 face export from Studio.")
    faces = {}
    for element in model.by_type("IfcBuildingElementProxy"):
        if not (element.ObjectType or "").startswith("Plane ("):
            continue
        if not any(
            rel.RelatingPropertyDefinition.is_a("IfcPropertySet")
            and rel.RelatingPropertyDefinition.Name == "OPS_ScanGeometry"
            for rel in element.IsDefinedBy
        ):
            continue
        if not element.Tag or not element.Tag.isascii() or not element.Tag.isdecimal():
            raise ValueError(f"Missing numeric face tag on {element.GlobalId}.")
        number = int(element.Tag)
        if number in faces:
            raise ValueError(f"Face {number} occurs more than once in this IFC.")
        faces[number] = element
    if not faces:
        raise ValueError("No unclassified Studio planes found.")

    assignments = {}
    for kind, numbers in (("IfcWall", args.wall), ("IfcSlab", args.slab)):
        for number in numbers:
            if number not in faces:
                raise ValueError(f"Face {number} is not an unclassified Studio plane.")
            if number in assignments:
                raise ValueError(f"Face {number} was assigned more than once.")
            if faces[number].IsTypedBy:
                raise ValueError(f"Face {number} already has an IFC type assignment.")
            assignments[number] = kind

    if not assignments and args.output is None:
        for number, face in sorted(faces.items()):
            print(f"{number}\t{face.Name}\t{face.GlobalId}")
        return
    if not assignments or args.output is None:
        raise ValueError("Specify an output filename and at least one --wall or --slab.")
    destination = args.input.with_name(args.output)
    if destination.suffix.lower() != ".ifc":
        raise ValueError("Use an .ifc output filename.")
    if destination.exists() and destination.samefile(args.input):
        raise ValueError("Choose a different filename for the classified IFC.")

    for number, kind in assignments.items():
        reassign_class(model, product=faces[number], ifc_class=kind, predefined_type="NOTDEFINED")
    with TemporaryDirectory(dir=destination.parent) as directory:
        temporary = Path(directory) / "classified.ifc"
        model.write(temporary)
        temporary.chmod(0o600)
        temporary.replace(destination)
    print(f"Wrote {destination} with {len(assignments)} classified faces.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="IFC exported by Studio")
    parser.add_argument("--output", help="output filename beside the input IFC")
    parser.add_argument("--wall", type=int, nargs="+", action="extend", default=[], metavar="FACE")
    parser.add_argument("--slab", type=int, nargs="+", action="extend", default=[], metavar="FACE")
    args = parser.parse_args()
    try:
        import ifcopenshell
        from ifcopenshell.api.root import reassign_class
    except ImportError:
        parser.exit(1, "Install IfcOpenShell with: python -m pip install ifcopenshell\n")
    try:
        classify(args, ifcopenshell.open(args.input), reassign_class)
    except (ifcopenshell.Error, ValueError, RuntimeError, OSError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()

using System.IO.Compression;
using System.Text.Json;
using System.Xml.Linq;
using DocumentFormat.OpenXml;
using DocumentFormat.OpenXml.Packaging;
using DocumentFormat.OpenXml.Wordprocessing;

namespace Navigator.WordAdapter;

internal static class Program
{
    private static readonly JsonSerializerOptions Json = new()
    {
        PropertyNamingPolicy = null,
        WriteIndented = false
    };

    public static async Task<int> Main()
    {
        try
        {
            var request = JsonSerializer.Deserialize<AdapterRequest>(
                await Console.In.ReadToEndAsync(), Json);
            if (request is null || request.protocol_version != Protocol.Version)
            {
                Write(new AdapterResponse(Protocol.Version, false, null,
                    Diagnostic.Protocol()));
                return 0;
            }

            byte[] bytes;
            try
            {
                bytes = Convert.FromBase64String(request.bytes_base64);
            }
            catch (FormatException)
            {
                Write(new AdapterResponse(Protocol.Version, false, null,
                    Diagnostic.Corrupt("package")));
                return 0;
            }

            Write(WordPackageParser.Parse(bytes));
            return 0;
        }
        catch (Exception)
        {
            // Exception messages can contain document data. The process
            // protocol carries a stable code instead of the exception text.
            Write(new AdapterResponse(Protocol.Version, false, null,
                Diagnostic.Corrupt("package")));
            return 0;
        }
    }

    private static void Write(AdapterResponse response)
    {
        Console.Write(JsonSerializer.Serialize(response, Json));
    }
}

internal static class Protocol
{
    public const int Version = 1;
}

internal sealed record AdapterRequest(int protocol_version, string bytes_base64);

internal sealed record AdapterResponse(
    int protocol_version,
    bool ok,
    object? document,
    Diagnostic? diagnostic);

internal sealed record Diagnostic(string code, string severity, string anchor)
{
    public static Diagnostic Protocol() =>
        new("protocol_version", "error", "request");

    public static Diagnostic Corrupt(string anchor) =>
        new("corrupt_package", "error", anchor);

    public static Diagnostic Rejected(string code, string anchor) =>
        new(code, "error", anchor);
}

internal static class WordPackageParser
{
    public static AdapterResponse Parse(byte[] bytes)
    {
        var safety = PackageSafety.Validate(bytes);
        if (safety is not null)
        {
            return new AdapterResponse(Protocol.Version, false, null, safety);
        }

        try
        {
            using var stream = new MemoryStream(bytes, writable: false);
            using var package = WordprocessingDocument.Open(stream, false);
            var main = package.MainDocumentPart;
            if (main?.Document?.Body is null)
            {
                return new AdapterResponse(Protocol.Version, false, null,
                    Diagnostic.Rejected("missing_main_document", "package"));
            }

            var inventory = PackageInventory.Build(package);
            if (inventory.External)
            {
                return new AdapterResponse(Protocol.Version, false, null,
                    Diagnostic.Rejected("external_relationship", "package"));
            }

            var revision = RevisionSupport.FindUnsupported(package);
            if (revision is not null)
            {
                return new AdapterResponse(Protocol.Version, false, null,
                    Diagnostic.Rejected("unsupported_revision", revision));
            }

            var reader = new StoryReader(StyleNumbering.Build(main.StyleDefinitionsPart?.Styles));
            var stories = new List<object>
            {
                reader.Story("main_document", main.Uri.ToString(), main.Document.Body)
            };

            foreach (var header in main.HeaderParts)
            {
                if (header.Header is not null)
                {
                    stories.Add(reader.Story("header", header.Uri.ToString(), header.Header));
                    stories.AddRange(reader.TextBoxes("text_box", header.Uri.ToString(), header.Header));
                }
            }
            foreach (var footer in main.FooterParts)
            {
                if (footer.Footer is not null)
                {
                    stories.Add(reader.Story("footer", footer.Uri.ToString(), footer.Footer));
                    stories.AddRange(reader.TextBoxes("text_box", footer.Uri.ToString(), footer.Footer));
                }
            }
            if (main.FootnotesPart?.Footnotes is not null)
            {
                stories.Add(reader.Story("footnotes", main.FootnotesPart.Uri.ToString(),
                    main.FootnotesPart.Footnotes));
            }
            if (main.EndnotesPart?.Endnotes is not null)
            {
                stories.Add(reader.Story("endnotes", main.EndnotesPart.Uri.ToString(),
                    main.EndnotesPart.Endnotes));
            }

            var comments = new List<object>();
            if (main.WordprocessingCommentsPart?.Comments is not null)
            {
                var commentBlocks = new List<object>();
                foreach (var comment in main.WordprocessingCommentsPart.Comments.Elements<Comment>())
                {
                    var blocks = reader.Blocks(comment);
                    comments.Add(new { id = comment.Id?.Value ?? string.Empty, blocks });
                    commentBlocks.AddRange(blocks);
                }
                stories.Add(new
                {
                    kind = "comments",
                    part_uri = main.WordprocessingCommentsPart.Uri.ToString(),
                    blocks = commentBlocks
                });
            }

            stories.AddRange(reader.TextBoxes("text_box", main.Uri.ToString(), main.Document.Body));
            return Success(package, inventory, stories, reader, comments);
        }
        catch (OpenXmlPackageException)
        {
            return new AdapterResponse(Protocol.Version, false, null,
                Diagnostic.Rejected("corrupt_package", "package"));
        }
        catch (InvalidDataException)
        {
            return new AdapterResponse(Protocol.Version, false, null,
                Diagnostic.Rejected("corrupt_package", "package"));
        }
    }

    private static AdapterResponse Success(
        WordprocessingDocument package,
        PackageInventory inventory,
        List<object> stories,
        StoryReader reader,
        List<object> comments)
    {
        var main = package.MainDocumentPart!;
        var styles = main.StyleDefinitionsPart?.Styles?.Elements<Style>()
            .Select(style => new
            {
                id = style.StyleId?.Value ?? string.Empty,
                style_type = style.Type?.Value.ToString(),
                based_on = style.BasedOn?.Val?.Value,
                next_style = style.NextParagraphStyle?.Val?.Value
            }).Cast<object>().ToList() ?? new List<object>();
        var numberingRoot = main.NumberingDefinitionsPart?.Numbering;
        var abstractNumbers = numberingRoot?.Elements<AbstractNum>()
            .ToDictionary(number => number.AbstractNumberId?.Value ?? -1)
            ?? new Dictionary<int, AbstractNum>();
        var numbering = numberingRoot?.Elements<NumberingInstance>()
            .Select(number =>
            {
                var abstractId = number.AbstractNumId?.Val?.Value;
                var abstractNumber = abstractId is not null
                    && abstractNumbers.TryGetValue(abstractId.Value, out var found)
                    ? found
                    : null;
                var overrides = number.Elements<LevelOverride>()
                    .ToDictionary(value => value.LevelIndex?.Value ?? 0);
                var levelDefinitions = abstractNumber?.Elements<Level>()
                    .Select(level =>
                    {
                        var levelIndex = level.LevelIndex?.Value ?? 0;
                        overrides.TryGetValue(levelIndex, out var levelOverride);
                        var overrideLevel = levelOverride?.GetFirstChild<Level>();
                        var overrideStart = UInt(
                            levelOverride?.GetFirstChild<StartOverrideNumberingValue>());
                        return new
                        {
                            level = levelIndex,
                            number_format = Value(overrideLevel, "numFmt")
                                ?? Value(level, "numFmt")
                                ?? string.Empty,
                            level_text = Value(overrideLevel, "lvlText")
                                ?? Value(level, "lvlText")
                                ?? string.Empty,
                            start = uint.TryParse(Value(level, "start"), out var start)
                                ? start
                                : 1U,
                            restart_level = Byte(Value(level, "lvlRestart")),
                            style_id = Value(level, "pStyle"),
                            override_start = overrideStart
                        };
                    }).Cast<object>().ToList() ?? new List<object>();
                return (object)new
                {
                    numbering_id = number.NumberID?.Value.ToString() ?? string.Empty,
                    abstract_numbering_id = abstractId?.ToString(),
                    levels = abstractNumber?.Elements<Level>()
                        .Select(level => Value(level, "lvlText") ?? string.Empty)
                        .ToList() ?? new List<string>(),
                    level_definitions = levelDefinitions
                };
            }).ToList() ?? new List<object>();

        var model = new
        {
            protocol_version = Protocol.Version,
            package = inventory.Value,
            stories,
            styles,
            numbering,
            comments,
            diagnostics = new List<object>(),
            revision_nodes = reader.Revisions
        };
        return new AdapterResponse(Protocol.Version, true, model, null);
    }

    private static string? Value(OpenXmlElement? element, string localName) => element?
        .ChildElements
        .FirstOrDefault(child => child.LocalName == localName)?
        .GetAttributes()
        .FirstOrDefault(attribute => attribute.LocalName == "val")?.Value;

    private static byte? Byte(string? value) => byte.TryParse(value, out var parsed) ? parsed : null;

    private static uint? UInt(OpenXmlElement? element) =>
        uint.TryParse(element?.GetAttributes()
            .FirstOrDefault(attribute => attribute.LocalName == "val")?.Value, out var parsed)
            ? parsed
            : null;
}

internal static class PackageSafety
{
    private const int MaxPackageBytes = 100 * 1024 * 1024;

    public static Diagnostic? Validate(byte[] bytes)
    {
        if (bytes.Length == 0 || bytes.Length > MaxPackageBytes)
        {
            return Diagnostic.Corrupt("package");
        }
        if (bytes.Length >= 8 && bytes[..8].SequenceEqual(new byte[]
            { 0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1 }))
        {
            return Diagnostic.Rejected("encrypted_package", "package");
        }

        try
        {
            using var archive = new ZipArchive(new MemoryStream(bytes), ZipArchiveMode.Read);
            var names = archive.Entries.Select(entry => entry.FullName.Replace('\\', '/')).ToList();
            var contentTypes = archive.GetEntry("[Content_Types].xml");
            if (contentTypes is null)
            {
                return Diagnostic.Corrupt("package");
            }
            using (var contentTypeStream = contentTypes.Open())
            {
                var contentTypeDocument = XDocument.Load(contentTypeStream);
                if (contentTypeDocument.Descendants().Any(element =>
                    element.Attribute("ContentType")?.Value.Contains(
                        "macroEnabled", StringComparison.OrdinalIgnoreCase) == true))
                {
                    return Diagnostic.Rejected("macro_enabled_package", "package");
                }
            }
            foreach (var name in names)
            {
                if (name.StartsWith('/') || name.Split('/').Any(segment => segment is "." or ".."))
                {
                    return Diagnostic.Rejected("escaping_package", "package");
                }
                if (name.Contains('\0', StringComparison.Ordinal))
                {
                    return Diagnostic.Corrupt("package");
                }
                if (name.Equals("word/vbaProject.bin", StringComparison.OrdinalIgnoreCase)
                    || name.Equals("word/vbaData.xml", StringComparison.OrdinalIgnoreCase))
                {
                    return Diagnostic.Rejected("macro_enabled_package", "package");
                }
                if (name.Equals("EncryptedPackage", StringComparison.OrdinalIgnoreCase)
                    || name.Equals("EncryptionInfo", StringComparison.OrdinalIgnoreCase))
                {
                    return Diagnostic.Rejected("encrypted_package", "package");
                }
            }

            foreach (var entry in archive.Entries.Where(entry =>
                entry.FullName.EndsWith(".rels", StringComparison.OrdinalIgnoreCase)))
            {
                using var stream = entry.Open();
                var relationships = XDocument.Load(stream);
                foreach (var relationship in relationships.Descendants().Where(element =>
                    element.Name.LocalName == "Relationship"))
                {
                    var mode = relationship.Attribute("TargetMode")?.Value;
                    var target = relationship.Attribute("Target")?.Value;
                    if (string.Equals(mode, "External", StringComparison.OrdinalIgnoreCase)
                        || target?.Contains("://", StringComparison.Ordinal) == true
                        || target is not null && EscapesPackage(entry.FullName, target))
                    {
                        return Diagnostic.Rejected(
                            mode?.Equals("External", StringComparison.OrdinalIgnoreCase) == true
                                ? "external_relationship"
                                : "escaping_package",
                            "relationship");
                    }
                }
            }

            if (!names.Contains("[Content_Types].xml") || !names.Contains("word/document.xml"))
            {
                return Diagnostic.Corrupt("package");
            }
            return null;
        }
        catch (InvalidDataException)
        {
            return Diagnostic.Corrupt("package");
        }
    }

    private static bool EscapesPackage(string relationshipPart, string target)
    {
        var source = relationshipPart.Replace('\\', '/');
        var absolute = target.StartsWith("/", StringComparison.Ordinal);
        if (source.StartsWith("_rels/", StringComparison.Ordinal))
        {
            source = source[6..];
        }
        else
        {
            var marker = source.IndexOf("/_rels/", StringComparison.Ordinal);
            if (marker >= 0)
            {
                source = source[..marker] + "/" + source[(marker + 7)..];
            }
        }
        source = source.EndsWith(".rels", StringComparison.Ordinal)
            ? source[..^5]
            : source;
        var slash = source.LastIndexOf('/');
        var segments = absolute || slash < 0
            ? new List<string>()
            : source[..slash].Split('/', StringSplitOptions.RemoveEmptyEntries).ToList();
        foreach (var segment in target.Split('/', StringSplitOptions.RemoveEmptyEntries))
        {
            if (segment == ".")
            {
                continue;
            }
            if (segment == "..")
            {
                if (segments.Count == 0)
                {
                    return true;
                }
                segments.RemoveAt(segments.Count - 1);
                continue;
            }
            segments.Add(segment);
        }
        return false;
    }
}

internal sealed class PackageInventory
{
    public bool External { get; private init; }
    public object Value { get; private init; } = new
    {
        parts = new List<object>(),
        relationships = new List<object>()
    };

    public static PackageInventory Build(WordprocessingDocument package)
    {
        var parts = new List<object>();
        var relationships = new List<object>();
        var seen = new HashSet<string>(StringComparer.Ordinal);
        var external = false;

        void Visit(OpenXmlPart part, string? relationshipType)
        {
            var uri = part.Uri.ToString();
            if (!seen.Add(uri))
            {
                return;
            }
            parts.Add(new
            {
                uri,
                content_type = part.ContentType,
                relationship_type = relationshipType
            });
            foreach (var child in part.Parts)
            {
                var childUri = child.OpenXmlPart.Uri.ToString();
                relationships.Add(new
                {
                    source_uri = uri,
                    relationship_id = child.RelationshipId,
                    target_uri = childUri,
                    relationship_type = child.OpenXmlPart.RelationshipType,
                    external = false
                });
                Visit(child.OpenXmlPart, child.OpenXmlPart.RelationshipType);
            }
            foreach (var relation in part.ExternalRelationships)
            {
                external = true;
                relationships.Add(new
                {
                    source_uri = uri,
                    relationship_id = relation.Id,
                    target_uri = string.Empty,
                    relationship_type = relation.RelationshipType,
                    external = true
                });
            }
        }

        foreach (var root in package.Parts)
        {
            relationships.Add(new
            {
                source_uri = string.Empty,
                relationship_id = root.RelationshipId,
                target_uri = root.OpenXmlPart.Uri.ToString(),
                relationship_type = root.OpenXmlPart.RelationshipType,
                external = false
            });
            Visit(root.OpenXmlPart, root.OpenXmlPart.RelationshipType);
        }
        foreach (var relation in package.ExternalRelationships)
        {
            external = true;
            relationships.Add(new
            {
                source_uri = string.Empty,
                relationship_id = relation.Id,
                target_uri = string.Empty,
                relationship_type = relation.RelationshipType,
                external = true
            });
        }

        return new PackageInventory
        {
            External = external,
            Value = new { parts, relationships }
        };
    }
}

internal static class RevisionSupport
{
    private static readonly Dictionary<string, string> Supported = new(StringComparer.Ordinal)
    {
        ["ins"] = "insertion", ["del"] = "deletion",
        ["moveFrom"] = "move_from", ["moveTo"] = "move_to",
        ["pPrChange"] = "paragraph_properties", ["rPrChange"] = "run_properties",
        ["tblPrChange"] = "table_properties", ["tblGridChange"] = "table_grid",
        ["trPrChange"] = "table_row_properties", ["tcPrChange"] = "table_cell_properties",
        ["numPrChange"] = "numbering", ["sectPrChange"] = "section_properties",
        ["numberingChange"] = "numbering", ["cellIns"] = "insertion",
        ["cellDel"] = "deletion", ["cellMerge"] = "table_properties",
        ["customXmlIns"] = "insertion", ["customXmlDel"] = "deletion",
        ["customXmlMoveFrom"] = "move_from", ["customXmlMoveTo"] = "move_to"
    };

    public static string? FindUnsupported(WordprocessingDocument package)
    {
        foreach (var part in AllParts(package))
        {
            OpenXmlElement? root;
            try
            {
                root = part.RootElement;
            }
            catch (Exception)
            {
                return "package";
            }
            if (root is null)
            {
                continue;
            }
            foreach (var element in root.Descendants())
            {
                if (element.NamespaceUri != "http://schemas.openxmlformats.org/wordprocessingml/2006/main")
                {
                    continue;
                }
                if (IsRevision(element.LocalName) && !Supported.ContainsKey(element.LocalName))
                {
                    return part.Uri + ":" + element.LocalName;
                }
            }
        }
        return null;
    }

    public static string? Kind(string localName) =>
        Supported.TryGetValue(localName, out var value) ? value : null;

    private static bool IsRevision(string localName) =>
        localName is "ins" or "del" or "moveFrom" or "moveTo"
            || localName.EndsWith("Change", StringComparison.Ordinal)
            || localName.StartsWith("customXml", StringComparison.Ordinal)
            || localName is "cellIns" or "cellDel" or "cellMerge";

    private static IEnumerable<OpenXmlPart> AllParts(WordprocessingDocument package)
    {
        var seen = new HashSet<string>(StringComparer.Ordinal);
        IEnumerable<OpenXmlPart> Walk(OpenXmlPart part)
        {
            if (!seen.Add(part.Uri.ToString()))
            {
                yield break;
            }
            yield return part;
            foreach (var child in part.Parts.Select(pair => pair.OpenXmlPart))
            {
                foreach (var nested in Walk(child))
                {
                    yield return nested;
                }
            }
        }
        foreach (var root in package.Parts)
        {
            foreach (var part in Walk(root.OpenXmlPart))
            {
                yield return part;
            }
        }
    }
}

/// Style-linked list numbering. A numbered paragraph frequently carries no
/// `w:numPr` of its own: the reference lives on its paragraph style, or on a
/// style that style is based on. Resolving the `w:basedOn` chain once, up
/// front, is what keeps those paragraphs from importing as ordinary text.
internal static class StyleNumbering
{
    internal sealed record Reference(string? NumberingId, string? Level);

    internal static IReadOnlyDictionary<string, Reference> Build(Styles? styles)
    {
        var resolved = new Dictionary<string, Reference>(StringComparer.Ordinal);
        if (styles is null)
        {
            return resolved;
        }
        var byId = new Dictionary<string, Style>(StringComparer.Ordinal);
        foreach (var style in styles.Elements<Style>())
        {
            var id = style.StyleId?.Value;
            if (id is not null)
            {
                byId[id] = style;
            }
        }
        foreach (var id in byId.Keys)
        {
            var reference = Resolve(id, byId, new HashSet<string>(StringComparer.Ordinal));
            if (reference is not null)
            {
                resolved[id] = reference;
            }
        }
        return resolved;
    }

    private static Reference? Resolve(
        string id,
        IReadOnlyDictionary<string, Style> byId,
        ISet<string> seen)
    {
        // A malformed package can point `w:basedOn` back at an ancestor; the
        // visited set makes that a missing reference rather than a hang.
        if (!seen.Add(id) || !byId.TryGetValue(id, out var style))
        {
            return null;
        }
        var numbering = style.StyleParagraphProperties?.NumberingProperties;
        var numberingId = numbering?.NumberingId?.Val?.Value.ToString();
        var level = numbering?.NumberingLevelReference?.Val?.Value.ToString();
        var basedOn = style.BasedOn?.Val?.Value;
        var inherited = basedOn is null ? null : Resolve(basedOn, byId, seen);
        numberingId ??= inherited?.NumberingId;
        level ??= inherited?.Level;
        return numberingId is null && level is null ? null : new Reference(numberingId, level);
    }
}

internal sealed class StoryReader
{
    private readonly IReadOnlyDictionary<string, StyleNumbering.Reference> _styleNumbering;
    private readonly Dictionary<string, int> _ordinals = new();

    public StoryReader(IReadOnlyDictionary<string, StyleNumbering.Reference> styleNumbering) =>
        _styleNumbering = styleNumbering;

    public List<object> Revisions { get; } = new();

    public object Story(string kind, string partUri, OpenXmlElement root) =>
        new { kind, part_uri = partUri, blocks = Blocks(root, partUri) };

    /// A block ordinal that keeps counting across nested containers, so a
    /// paragraph without a `w14:paraId` inside a table cell never collides
    /// with one in a sibling cell or at the top level of the same part.
    private int NextOrdinal(string partUri)
    {
        _ordinals.TryGetValue(partUri, out var next);
        _ordinals[partUri] = next + 1;
        return next;
    }

    public IEnumerable<object> TextBoxes(string kind, string partUri, OpenXmlElement root) =>
        root.Descendants().Where(element => element.LocalName == "txbxContent")
            .Select(element => Story(kind, partUri, element));

    public List<object> Blocks(OpenXmlElement root) => Blocks(root, root.LocalName);

    private List<object> Blocks(OpenXmlElement root, string partUri)
    {
        var blocks = new List<object>();
        foreach (var child in root.ChildElements)
        {
            switch (child.LocalName)
            {
                case "p":
                    blocks.Add(Paragraph((Paragraph)child, partUri, NextOrdinal(partUri)));
                    break;
                case "tbl":
                    blocks.Add(Table((Table)child, partUri, NextOrdinal(partUri)));
                    break;
                case "sectPr":
                    blocks.Add(new
                    {
                        kind = "section_break",
                        break_kind = "page",
                        anchor = $"{partUri}:section-break:{NextOrdinal(partUri)}"
                    });
                    break;
                case "txbxContent":
                    blocks.AddRange(Blocks(child, partUri));
                    break;
                case "footnote":
                case "endnote":
                case "comment":
                    blocks.AddRange(Blocks(child, partUri));
                    break;
            }
        }
        return blocks;
    }

    private object Paragraph(Paragraph paragraph, string partUri, int ordinal)
    {
        var properties = paragraph.ParagraphProperties;
        var styleId = properties?.ParagraphStyleId?.Val?.Value;
        var numbering = NumberingFor(properties, styleId);
        var revisions = RevisionProperties(properties);
        var paragraphId = paragraph.GetAttributes()
            .Where(attribute => attribute.LocalName == "paraId")
            .Select(attribute => attribute.Value)
            .FirstOrDefault();
        return new
        {
            kind = "paragraph",
            anchor = paragraphId is null
                ? $"{partUri}:paragraph:{ordinal}"
                : $"{partUri}:paragraph:{paragraphId}",
            style_id = styleId,
            numbering = numbering is null ? null : new
            {
                numbering_id = numbering.NumberingId ?? string.Empty,
                level = numbering.Level
            },
            nodes = Inlines(paragraph),
            revisions
        };
    }

    /// Word paragraphs carry numbering either directly on the paragraph or
    /// through the paragraph style, and OOXML lets the two supply different
    /// halves of the same reference. Direct properties win field by field;
    /// whatever the paragraph omits falls back to the resolved style chain,
    /// so a style-linked numbered paragraph is an outline unit rather than
    /// ordinary prose.
    private StyleNumbering.Reference? NumberingFor(ParagraphProperties? properties, string? styleId)
    {
        var direct = properties?.NumberingProperties;
        var inherited = styleId is not null && _styleNumbering.TryGetValue(styleId, out var found)
            ? found
            : null;
        if (direct is null)
        {
            return inherited;
        }
        var numberingId = direct.NumberingId?.Val?.Value.ToString() ?? inherited?.NumberingId;
        var level = direct.NumberingLevelReference?.Val?.Value.ToString() ?? inherited?.Level;
        return numberingId is null && level is null
            ? null
            : new StyleNumbering.Reference(numberingId, level);
    }

    private object Table(Table table, string partUri, int ordinal)
    {
        var revisions = RevisionProperties(table.TableProperties);
        var rows = table.Elements<TableRow>().Select(row => new
        {
            cells = row.Elements<TableCell>()
                .Select(cell => new { blocks = Blocks(cell, partUri) }).ToList()
        }).ToList();
        return new
        {
            kind = "table",
            anchor = $"{partUri}:table:{ordinal}",
            style_id = table.TableProperties?.TableStyle?.Val?.Value,
            rows,
            revisions
        };
    }

    private List<object> Inlines(OpenXmlElement root)
    {
        var nodes = new List<object>();
        foreach (var child in root.ChildElements)
        {
            switch (child.LocalName)
            {
                case "r":
                    nodes.AddRange(Run((Run)child));
                    break;
                case "hyperlink":
                    nodes.Add(new
                    {
                        kind = "hyperlink",
                        relationship_id = Attribute(child, "id",
                            "http://schemas.openxmlformats.org/officeDocument/2006/relationships"),
                        anchor = Attribute(child, "anchor",
                            "http://schemas.openxmlformats.org/wordprocessingml/2006/main"),
                        children = Inlines(child)
                    });
                    break;
                case "fldSimple":
                    nodes.Add(new
                    {
                        kind = "field",
                        field_kind = "simple",
                        instruction = Attribute(child, "instr", child.NamespaceUri),
                        children = Inlines(child)
                    });
                    break;
                case "bookmarkStart":
                    nodes.Add(new
                    {
                        kind = "bookmark_start",
                        id = Attribute(child, "id", child.NamespaceUri) ?? string.Empty,
                        name = Attribute(child, "name", child.NamespaceUri)
                    });
                    break;
                case "bookmarkEnd":
                    nodes.Add(new
                    {
                        kind = "bookmark_end",
                        id = Attribute(child, "id", child.NamespaceUri) ?? string.Empty
                    });
                    break;
                case "commentRangeStart":
                    nodes.Add(new
                    {
                        kind = "comment_range_start",
                        id = Attribute(child, "id", child.NamespaceUri) ?? string.Empty
                    });
                    break;
                case "commentRangeEnd":
                    nodes.Add(new
                    {
                        kind = "comment_range_end",
                        id = Attribute(child, "id", child.NamespaceUri) ?? string.Empty
                    });
                    break;
                case "commentReference":
                    nodes.Add(new
                    {
                        kind = "comment_reference",
                        id = Attribute(child, "id", child.NamespaceUri) ?? string.Empty
                    });
                    break;
                case "ins":
                case "del":
                case "moveFrom":
                case "moveTo":
                    nodes.Add(Revision(child));
                    break;
                case "smartTag":
                case "sdt":
                case "customXml":
                    nodes.AddRange(Inlines(child));
                    break;
            }
        }
        return nodes;
    }

    private static string? Attribute(OpenXmlElement element, string localName, string namespaceUri) =>
        element.GetAttributes()
            .Where(attribute => attribute.LocalName == localName
                && attribute.NamespaceUri == namespaceUri)
            .Select(attribute => attribute.Value)
            .FirstOrDefault();

    private IEnumerable<object> Run(Run run)
    {
        var style = run.RunProperties?.RunStyle?.Val?.Value;
        foreach (var child in run.ChildElements)
        {
            switch (child.LocalName)
            {
                case "instrText":
                    yield return new
                    {
                        kind = "field",
                        field_kind = "complex",
                        instruction = child.InnerText,
                        children = new List<object>()
                    };
                    break;
                case "fldChar":
                    yield return new
                    {
                        kind = "field",
                        field_kind = "complex",
                        instruction = Attribute(child, "fldCharType", child.NamespaceUri),
                        children = new List<object>()
                    };
                    break;
                case "t":
                case "delText":
                    yield return new
                    {
                        kind = "text",
                        text = child.InnerText,
                        style_id = style,
                        revision = (string?)null
                    };
                    break;
                case "tab":
                    yield return new { kind = "tab", style_id = style };
                    break;
                case "br":
                    yield return new
                    {
                        kind = "break",
                        break_kind = child.GetAttribute("type", child.NamespaceUri).Value switch
                        {
                            "page" => "page",
                            "column" => "column",
                            _ => "line"
                        }
                    };
                    break;
                case "lastRenderedPageBreak":
                    yield return new { kind = "break", break_kind = "page" };
                    break;
            }
        }
    }

    private object Revision(OpenXmlElement element)
    {
        var kind = RevisionSupport.Kind(element.LocalName) ?? "unsupported";
        var revision = new
        {
            kind,
            id = Attribute(element, "id", element.NamespaceUri),
            anchor = element.LocalName
        };
        Revisions.Add(revision);
        return new { kind = "revision", revision, children = Inlines(element) };
    }

    private List<object> RevisionProperties(OpenXmlElement? properties)
    {
        if (properties is null)
        {
            return new List<object>();
        }
        var revisions = new List<object>();
        foreach (var element in properties.Descendants())
        {
            var kind = RevisionSupport.Kind(element.LocalName);
            if (kind is null)
            {
                continue;
            }
            var revision = new
            {
                kind,
                id = Attribute(element, "id", element.NamespaceUri),
                anchor = element.LocalName
            };
            revisions.Add(revision);
            Revisions.Add(revision);
        }
        return revisions;
    }

    private static string? Value(OpenXmlElement? element, string localName) => element?
        .ChildElements
        .FirstOrDefault(child => child.LocalName == localName)?
        .GetAttributes()
        .FirstOrDefault(attribute => attribute.LocalName == "val")?.Value;

    private static byte? Byte(string? value) => byte.TryParse(value, out var parsed) ? parsed : null;

    private static uint? UInt(OpenXmlElement? element) =>
        uint.TryParse(element?.GetAttributes()
            .FirstOrDefault(attribute => attribute.LocalName == "val")?.Value, out var parsed)
            ? parsed
            : null;
}

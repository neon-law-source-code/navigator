using System.IO.Compression;
using System.Text.Json;
using DocumentFormat.OpenXml;
using DocumentFormat.OpenXml.Packaging;
using DocumentFormat.OpenXml.Wordprocessing;
using Xunit;

namespace Navigator.WordAdapter;

public sealed class WordAdapterTests
{
    [Fact]
    public void synthetic_package_preserves_stories_structure_styles_numbering_and_revisions()
    {
        var response = WordPackageParser.Parse(SyntheticDocx.Create());
        var json = JsonSerializer.Serialize(response);
        using var document = JsonDocument.Parse(json);
        var root = document.RootElement;

        Assert.True(root.GetProperty("ok").GetBoolean(), json);
        var model = root.GetProperty("document");
        Assert.Contains(model.GetProperty("stories").EnumerateArray(), story =>
            story.GetProperty("kind").GetString() == "main_document");
        Assert.Contains(model.GetProperty("stories").EnumerateArray(), story =>
            story.GetProperty("kind").GetString() == "header");
        Assert.Contains(model.GetProperty("stories").EnumerateArray(), story =>
            story.GetProperty("kind").GetString() == "footer");
        Assert.Contains(model.GetProperty("stories").EnumerateArray(), story =>
            story.GetProperty("kind").GetString() == "footnotes");
        Assert.Contains(model.GetProperty("stories").EnumerateArray(), story =>
            story.GetProperty("kind").GetString() == "endnotes");
        Assert.Contains(model.GetProperty("stories").EnumerateArray(), story =>
            story.GetProperty("kind").GetString() == "footnotes"
                && story.GetProperty("blocks").EnumerateArray().Any(block =>
                    block.GetProperty("kind").GetString() == "paragraph"));
        Assert.Contains(model.GetProperty("stories").EnumerateArray(), story =>
            story.GetProperty("kind").GetString() == "endnotes"
                && story.GetProperty("blocks").EnumerateArray().Any(block =>
                    block.GetProperty("kind").GetString() == "paragraph"));
        Assert.Contains(model.GetProperty("stories").EnumerateArray(), story =>
            story.GetProperty("kind").GetString() == "comments");
        Assert.NotEmpty(model.GetProperty("package").GetProperty("parts").EnumerateArray());
        Assert.NotEmpty(model.GetProperty("package").GetProperty("relationships").EnumerateArray());
        Assert.Contains(model.GetProperty("package").GetProperty("relationships").EnumerateArray(), relation =>
            relation.GetProperty("source_uri").GetString() == string.Empty
                && relation.GetProperty("target_uri").GetString()?.EndsWith("/document.xml") == true);
        Assert.Contains(model.GetProperty("styles").EnumerateArray(), style =>
            style.GetProperty("id").GetString() == "SyntheticBody");
        Assert.Contains(model.GetProperty("numbering").EnumerateArray(), numbering =>
            numbering.GetProperty("numbering_id").GetString() == "7");
        Assert.Contains(model.GetProperty("revision_nodes").EnumerateArray(), revision =>
            revision.GetProperty("kind").GetString() == "insertion");
        Assert.Contains(model.GetProperty("revision_nodes").EnumerateArray(), revision =>
            revision.GetProperty("kind").GetString() == "deletion");
        Assert.Contains("\"kind\":\"bookmark_start\"", json);
        Assert.Contains("\"kind\":\"hyperlink\"", json);
        Assert.Contains("\"kind\":\"field\"", json);
        Assert.Contains("\"kind\":\"table\"", json);
        Assert.Contains("\"kind\":\"section_break\"", json);
    }

    [Fact]
    public void package_safety_rejects_external_relationships_without_fetching()
    {
        var bytes = SyntheticDocx.WithRelationshipTarget("https://example.invalid/resource");
        var response = WordPackageParser.Parse(bytes);
        var json = JsonSerializer.Serialize(response);
        using var document = JsonDocument.Parse(json);
        Assert.Equal("external_relationship",
            document.RootElement.GetProperty("diagnostic").GetProperty("code").GetString());
    }

    [Fact]
    public void package_safety_rejects_escaping_relationships()
    {
        var bytes = SyntheticDocx.WithRelationshipTarget("../../outside.xml");
        var response = WordPackageParser.Parse(bytes);
        var json = JsonSerializer.Serialize(response);
        using var document = JsonDocument.Parse(json);
        Assert.Equal("escaping_package",
            document.RootElement.GetProperty("diagnostic").GetProperty("code").GetString());
    }

    [Fact]
    public void package_safety_rejects_macro_entries()
    {
        var response = WordPackageParser.Parse(SyntheticDocx.WithEntry("word/vbaProject.bin"));
        var json = JsonSerializer.Serialize(response);
        using var document = JsonDocument.Parse(json);
        Assert.Equal("macro_enabled_package",
            document.RootElement.GetProperty("diagnostic").GetProperty("code").GetString());
    }

    [Fact]
    public void package_safety_rejects_macro_content_types()
    {
        var response = WordPackageParser.Parse(SyntheticDocx.WithMacroContentType());
        var json = JsonSerializer.Serialize(response);
        using var document = JsonDocument.Parse(json);
        Assert.Equal("macro_enabled_package",
            document.RootElement.GetProperty("diagnostic").GetProperty("code").GetString());
    }

    [Fact]
    public void package_safety_rejects_unsupported_revision_nodes()
    {
        var response = WordPackageParser.Parse(SyntheticDocx.WithUnsupportedRevision());
        var json = JsonSerializer.Serialize(response);
        using var document = JsonDocument.Parse(json);
        Assert.Equal("unsupported_revision",
            document.RootElement.GetProperty("diagnostic").GetProperty("code").GetString());
    }

    private static class SyntheticDocx
    {
        public static byte[] Create()
        {
            using var stream = new MemoryStream();
            using (var package = WordprocessingDocument.Create(
                stream, WordprocessingDocumentType.Document, true))
            {
                var main = package.AddMainDocumentPart();
                var body = new Body(
                    new Paragraph(
                        new ParagraphProperties(
                            new ParagraphStyleId { Val = "SyntheticBody" },
                            new NumberingProperties(
                                new NumberingLevelReference { Val = 0 },
                                new NumberingId { Val = 7 })),
                        new BookmarkStart { Id = "1", Name = "synthetic-bookmark" },
                        new Run(new Text("before ")),
                        new Hyperlink(new Run(new Text("linked"))) { Anchor = "synthetic-bookmark" },
                        new SimpleField(new Run(new Text("field-result"))) { Instruction = "PAGE" },
                        new InsertedRun(new Run(new Text("inserted"))) { Id = "2", Author = "fixture" },
                        new DeletedRun(new Run(new DeletedText("deleted"))) { Id = "3", Author = "fixture" },
                        new BookmarkEnd { Id = "1" },
                        new Run(new Break { Type = BreakValues.Page }),
                        new CommentReference { Id = "4" }),
                    new Table(
                        new TableRow(
                            new TableCell(new Paragraph(new Run(new Text("cell one")))),
                            new TableCell(new Paragraph(new Run(new Text("cell two"))))
                        )
                    ),
                    new SectionProperties());
                main.Document = new Document(body);

                var styles = main.AddNewPart<StyleDefinitionsPart>();
                styles.Styles = new Styles(new Style
                {
                    Type = StyleValues.Paragraph,
                    StyleId = "SyntheticBody",
                    BasedOn = new BasedOn { Val = "Normal" },
                    NextParagraphStyle = new NextParagraphStyle { Val = "Normal" }
                });

                var numbering = main.AddNewPart<NumberingDefinitionsPart>();
                numbering.Numbering = new Numbering(
                    new AbstractNum(new Level { LevelIndex = 0 }) { AbstractNumberId = 3 },
                    new NumberingInstance(new AbstractNumId { Val = 3 }) { NumberID = 7 });

                var header = main.AddNewPart<HeaderPart>();
                header.Header = new Header(new Paragraph(new Run(new Text("header"))));
                var footer = main.AddNewPart<FooterPart>();
                footer.Footer = new Footer(new Paragraph(new Run(new Text("footer"))));
                var footnotes = main.AddNewPart<FootnotesPart>();
                footnotes.Footnotes = new Footnotes(new Footnote(new Paragraph(new Run(new Text("footnote")))) { Id = 1 });
                var endnotes = main.AddNewPart<EndnotesPart>();
                endnotes.Endnotes = new Endnotes(new Endnote(new Paragraph(new Run(new Text("endnote")))) { Id = 1 });
                var comments = main.AddNewPart<WordprocessingCommentsPart>();
                comments.Comments = new Comments(new Comment(
                    new Paragraph(new Run(new Text("comment")))) { Id = "4" });

                main.Document.Save();
                styles.Styles.Save();
                numbering.Numbering.Save();
                header.Header.Save();
                footer.Footer.Save();
                footnotes.Footnotes.Save();
                endnotes.Endnotes.Save();
                comments.Comments.Save();
            }
            return stream.ToArray();
        }

        public static byte[] WithRelationshipTarget(string target)
        {
            using var stream = new MemoryStream();
            var mode = target.Contains("://", StringComparison.Ordinal)
                ? " TargetMode=\"External\""
                : string.Empty;
            using (var archive = new ZipArchive(stream, ZipArchiveMode.Create, true))
            {
                Add(archive, "[Content_Types].xml",
                    "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\" /></Types>");
                Add(archive, "word/document.xml",
                    "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body /></w:document>");
                Add(archive, "word/_rels/document.xml.rels",
                    $"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{target}\"{mode} /></Relationships>");
            }
            return stream.ToArray();
        }

        public static byte[] WithEntry(string name) => WithDocument(
            "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body /></w:document>",
            (name, "fixture"));

        public static byte[] WithMacroContentType() => WithDocumentAndMainContentType(
            "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body /></w:document>",
            "application/vnd.ms-word.document.macroEnabled.main+xml");

        public static byte[] WithUnsupportedRevision()
        {
            var source = Create();
            using var input = new MemoryStream(source);
            using var archive = new ZipArchive(input, ZipArchiveMode.Read);
            using var output = new MemoryStream();
            using (var rewritten = new ZipArchive(output, ZipArchiveMode.Create, true))
            {
                foreach (var entry in archive.Entries)
                {
                    var copy = rewritten.CreateEntry(entry.FullName);
                    using var sourceStream = entry.Open();
                    using var targetStream = copy.Open();
                    if (entry.FullName == "word/document.xml")
                    {
                        using var reader = new StreamReader(sourceStream);
                        var content = reader.ReadToEnd().Replace(
                            "</w:body>", "<w:fooChange /></w:body>", StringComparison.Ordinal);
                        using var writer = new StreamWriter(targetStream);
                        writer.Write(content);
                    }
                    else
                    {
                        sourceStream.CopyTo(targetStream);
                    }
                }
            }
            return output.ToArray();
        }

        public static byte[] WithDocument(string document, params (string name, string content)[] entries)
            => WithDocumentAndMainContentType(
                document,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
                entries);

        private static byte[] WithDocumentAndMainContentType(
            string document,
            string mainContentType,
            params (string name, string content)[] entries)
        {
            using var stream = new MemoryStream();
            using (var archive = new ZipArchive(stream, ZipArchiveMode.Create, true))
            {
                Add(archive, "[Content_Types].xml",
                    $"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Override PartName=\"/word/document.xml\" ContentType=\"{mainContentType}\" /></Types>");
                Add(archive, "_rels/.rels",
                    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\" /></Relationships>");
                Add(archive, "word/document.xml", document);
                foreach (var (name, content) in entries)
                {
                    Add(archive, name, content);
                }
            }
            return stream.ToArray();
        }

        private static void Add(ZipArchive archive, string name, string content)
        {
            var entry = archive.CreateEntry(name);
            using (var writer = new StreamWriter(entry.Open()))
            {
                writer.Write(content);
            }
        }
    }
}

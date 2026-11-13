import com.sun.source.util.JavacTask;
import javax.lang.model.element.*;
import javax.lang.model.type.*;
import javax.lang.model.util.*;
import javax.tools.*;
import java.io.IOException;
import java.nio.file.*;
import java.util.*;

/**
 * Generates base classes for the Paper API types the plugin host implements.
 *
 * <p>For every target type it writes a class that implements (or extends) it and defines
 * every abstract method, inherited ones included, as one that throws
 * {@code Unsupported.method(...)}. Hand-written host classes extend these and override only
 * what they really support, so a missing feature fails with a clear message instead of a
 * compile error or an {@code AbstractMethodError}.
 *
 * <p>Targets marked {@code value} are registry entries: the generated class also takes a
 * {@code NamespacedKey} and an ordinal and implements the members every registry entry
 * shares ({@code getKey}, {@code key}, {@code name}, {@code ordinal}, {@code compareTo},
 * {@code translationKey}, {@code getTranslationKey}, {@code equals}/{@code hashCode}/{@code toString}).
 *
 * <p>Reads class files only (javac's model of the classpath); nothing from the API is run.
 *
 * <pre>
 * java tools/StubGen.java &lt;classpath&gt; &lt;targets file&gt; &lt;output source root&gt;
 * </pre>
 *
 * Targets file, one per line: {@code <type> <generated class name> [abstract|final] [value]},
 * with {@code #} comments. Nested types use {@code $} ({@code org.bukkit.entity.Villager$Type}).
 */
public final class StubGen {
    private static final String STUB_PACKAGE = "io.papermc.paperrs.host.stub";

    private final Elements elements;
    private final Types types;
    private final TypeElement objectElement;

    private StubGen(Elements elements, Types types) {
        this.elements = elements;
        this.types = types;
        this.objectElement = elements.getTypeElement("java.lang.Object");
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 3) {
            System.err.println("usage: StubGen <classpath> <targets file> <output source root>");
            System.exit(2);
        }
        JavaCompiler compiler = ToolProvider.getSystemJavaCompiler();
        JavacTask task = (JavacTask) compiler.getTask(null, null, null, List.of("-proc:none", "-classpath", args[0]), null,
            List.of(new SimpleJavaFileObject(java.net.URI.create("string:///Dummy.java"), JavaFileObject.Kind.SOURCE) {
                @Override public CharSequence getCharContent(boolean ignore) { return "class Dummy {}"; }
            }));
        task.analyze();
        StubGen gen = new StubGen(task.getElements(), task.getTypes());
        Path out = Path.of(args[2]);
        int count = 0;
        for (String raw : Files.readAllLines(Path.of(args[1]))) {
            String line = raw.strip();
            if (line.isEmpty() || line.startsWith("#")) continue;
            String[] parts = line.split("\\s+");
            Set<String> flags = new HashSet<>(Arrays.asList(parts).subList(2, parts.length));
            gen.generate(parts[0], parts[1], flags, out);
            count++;
        }
        System.out.println("generated " + count + " classes into " + out);
    }

    // --- type rendering (without annotations) -------------------------------------------------

    private String render(TypeMirror t) {
        return switch (t.getKind()) {
            case BOOLEAN, BYTE, SHORT, INT, LONG, CHAR, FLOAT, DOUBLE -> t.getKind().name().toLowerCase(Locale.ROOT);
            case VOID -> "void";
            case ARRAY -> render(((ArrayType) t).getComponentType()) + "[]";
            case TYPEVAR -> ((TypeVariable) t).asElement().getSimpleName().toString();
            case WILDCARD -> {
                WildcardType w = (WildcardType) t;
                if (w.getExtendsBound() != null) yield "? extends " + render(w.getExtendsBound());
                if (w.getSuperBound() != null) yield "? super " + render(w.getSuperBound());
                yield "?";
            }
            case INTERSECTION -> {
                StringJoiner j = new StringJoiner(" & ");
                for (TypeMirror b : ((IntersectionType) t).getBounds()) j.add(render(b));
                yield j.toString();
            }
            case DECLARED -> {
                DeclaredType d = (DeclaredType) t;
                String name = ((TypeElement) d.asElement()).getQualifiedName().toString();
                if (d.getTypeArguments().isEmpty()) yield name;
                StringJoiner j = new StringJoiner(", ", "<", ">");
                for (TypeMirror a : d.getTypeArguments()) j.add(render(a));
                yield name + j;
            }
            default -> throw new IllegalStateException("cannot render " + t.getKind() + ": " + t);
        };
    }

    private String typeParameters(List<? extends TypeVariable> vars) {
        if (vars.isEmpty()) return "";
        StringJoiner j = new StringJoiner(", ", "<", "> ");
        for (TypeVariable v : vars) {
            String name = v.asElement().getSimpleName().toString();
            TypeMirror bound = v.getUpperBound();
            boolean isObject = bound.getKind() == TypeKind.DECLARED && ((TypeElement) ((DeclaredType) bound).asElement()).equals(objectElement);
            j.add(isObject ? name : name + " extends " + render(bound));
        }
        return j.toString();
    }

    // --- generation ---------------------------------------------------------------------------

    private record Abstract(ExecutableElement element, ExecutableType type) {}

    /** Abstract instance methods a concrete subtype must define, keyed by name and erasure. */
    private Map<String, Abstract> abstractMethods(TypeElement te, DeclaredType asType) {
        Map<String, Abstract> abstracts = new TreeMap<>();
        Set<String> concrete = new HashSet<>();
        for (Element e : elements.getAllMembers(te)) {
            if (e.getKind() != ElementKind.METHOD) continue;
            ExecutableElement m = (ExecutableElement) e;
            if (m.getModifiers().contains(Modifier.STATIC) || m.getModifiers().contains(Modifier.PRIVATE)) continue;
            if (m.getEnclosingElement().equals(objectElement)) continue;
            ExecutableType mt;
            try {
                mt = (ExecutableType) types.asMemberOf(asType, m);
            } catch (IllegalArgumentException ex) {
                mt = (ExecutableType) m.asType();
            }
            StringBuilder key = new StringBuilder(m.getSimpleName()).append('(');
            for (TypeMirror p : mt.getParameterTypes()) key.append(types.erasure(p)).append(',');
            String k = key.append(')').toString();
            if (k.equals("equals(java.lang.Object,)") || k.equals("hashCode()") || k.equals("toString()")) continue;
            if (!m.getModifiers().contains(Modifier.ABSTRACT)) {
                concrete.add(k);
                continue;
            }
            Abstract candidate = new Abstract(m, mt);
            Abstract existing = abstracts.get(k);
            // Several superinterfaces may declare the same method with covariant returns;
            // the implementation must use the most specific one.
            if (existing == null || types.isSubtype(types.erasure(mt.getReturnType()), types.erasure(existing.type().getReturnType()))) {
                abstracts.put(k, candidate);
            }
        }
        abstracts.keySet().removeAll(concrete);
        return abstracts;
    }

    private static String ownerName(TypeElement te) {
        // org.bukkit.entity.Villager.Type -> Villager.Type
        StringBuilder b = new StringBuilder(te.getSimpleName());
        Element enclosing = te.getEnclosingElement();
        while (enclosing instanceof TypeElement outer) {
            b.insert(0, outer.getSimpleName() + ".");
            enclosing = outer.getEnclosingElement();
        }
        return b.toString();
    }

    private void generate(String target, String className, Set<String> flags, Path outRoot) throws IOException {
        TypeElement te = elements.getTypeElement(target.replace('$', '.'));
        if (te == null) throw new IllegalArgumentException("type not found: " + target);
        boolean isInterface = te.getKind() == ElementKind.INTERFACE;
        boolean value = flags.contains("value");
        boolean isFinal = flags.contains("final");
        DeclaredType asType = (DeclaredType) te.asType();
        String owner = ownerName(te);
        boolean publicTarget = te.getModifiers().contains(Modifier.PUBLIC);
        String pkg = publicTarget ? STUB_PACKAGE : elements.getPackageOf(te).getQualifiedName().toString();

        StringBuilder src = new StringBuilder();
        src.append("// Generated by plugin-host/tools/StubGen.java from the Paper API class files. Do not edit;\n");
        src.append("// change tools/stubs.txt or override the methods in a subclass instead.\n");
        src.append("package ").append(pkg).append(";\n\n");
        src.append("import io.papermc.paperrs.host.Unsupported;\n\n");
        src.append("/** Every abstract member of {@code ").append(owner).append("}, unsupported until overridden. */\n");
        src.append("@SuppressWarnings({\"deprecation\", \"removal\", \"unchecked\", \"rawtypes\", \"unused\"})\n");
        src.append(publicTarget ? "public " : "").append(isFinal ? "final " : "abstract ").append("class ").append(className);
        List<? extends TypeParameterElement> tps = te.getTypeParameters();
        List<TypeVariable> classVars = new ArrayList<>();
        for (TypeParameterElement tp : tps) classVars.add((TypeVariable) tp.asType());
        src.append(typeParameters(classVars).stripTrailing());
        String targetRef = render(asType);
        src.append(isInterface ? " implements " : " extends ").append(targetRef).append(" {\n");

        if (value) {
            src.append("    protected final org.bukkit.NamespacedKey key;\n");
            src.append("    protected final int ordinal;\n");
            src.append("    protected final String translationKey;\n\n");
        }

        // Constructors: mirror the accessible constructors of an abstract superclass.
        if (!isInterface) {
            boolean any = false;
            for (Element e : te.getEnclosedElements()) {
                if (e.getKind() != ElementKind.CONSTRUCTOR || e.getModifiers().contains(Modifier.PRIVATE)) continue;
                ExecutableElement c = (ExecutableElement) e;
                any = true;
                List<String> params = new ArrayList<>();
                List<String> names = new ArrayList<>();
                int i = 0;
                for (VariableElement p : c.getParameters()) {
                    params.add(render(p.asType()) + " a" + i);
                    names.add("a" + i);
                    i++;
                }
                if (value) {
                    params.add(0, "org.bukkit.NamespacedKey key, int ordinal, String translationKey");
                }
                src.append(isFinal ? "    public " : "    protected ").append(className).append("(").append(String.join(", ", params)).append(") {\n");
                src.append("        super(").append(String.join(", ", names)).append(");\n");
                if (value) src.append("        this.key = key;\n        this.ordinal = ordinal;\n        this.translationKey = translationKey;\n");
                src.append("    }\n\n");
            }
            if (!any && value) throw new IllegalStateException(target + " has no accessible constructor");
        } else if (value) {
            src.append("    ").append(isFinal ? "public " : "protected ").append(className)
                .append("(org.bukkit.NamespacedKey key, int ordinal, String translationKey) {\n")
                .append("        this.key = key;\n        this.ordinal = ordinal;\n        this.translationKey = translationKey;\n    }\n\n");
        }

        for (Map.Entry<String, Abstract> entry : abstractMethods(te, asType).entrySet()) {
            ExecutableElement m = entry.getValue().element();
            ExecutableType mt = entry.getValue().type();
            String name = m.getSimpleName().toString();
            String body = value ? valueBody(name, mt) : null;
            List<String> params = new ArrayList<>();
            List<? extends TypeMirror> ptypes = mt.getParameterTypes();
            for (int i = 0; i < ptypes.size(); i++) {
                String rendered = render(ptypes.get(i));
                if (m.isVarArgs() && i == ptypes.size() - 1) rendered = rendered.substring(0, rendered.length() - 2) + "...";
                params.add(rendered + " p" + i);
            }
            String visibility = m.getModifiers().contains(Modifier.PROTECTED) ? "protected " : "public ";
            src.append("    @Override\n    ").append(visibility).append(typeParameters(mt.getTypeVariables()))
                .append(render(mt.getReturnType())).append(' ').append(name).append('(').append(String.join(", ", params)).append(')');
            if (!mt.getThrownTypes().isEmpty()) {
                StringJoiner j = new StringJoiner(", ", " throws ", "");
                for (TypeMirror t : mt.getThrownTypes()) j.add(render(t));
                src.append(j);
            }
            if (body != null) {
                src.append(" {\n        ").append(body).append("\n    }\n\n");
            } else {
                src.append(" {\n        throw Unsupported.method(\"").append(owner).append('#').append(name).append("\");\n    }\n\n");
            }
        }

        if (value) {
            src.append("    @Override\n    public boolean equals(Object o) {\n        return this == o;\n    }\n\n");
            src.append("    @Override\n    public int hashCode() {\n        return this.key.hashCode();\n    }\n\n");
            src.append("    @Override\n    public String toString() {\n        return \"").append(className).append("{\" + this.key + \"}\";\n    }\n");
        }
        src.append("}\n");

        Path file = outRoot.resolve(pkg.replace('.', '/')).resolve(className + ".java");
        Files.createDirectories(file.getParent());
        Files.writeString(file, src);
    }

    /** Implementation of a member every registry entry shares, or null to stub it. */
    private String valueBody(String name, ExecutableType mt) {
        int arity = mt.getParameterTypes().size();
        String ret = render(mt.getReturnType());
        return switch (name) {
            case "getKey" -> arity == 0 && ret.equals("org.bukkit.NamespacedKey") ? "return this.key;" : null;
            case "key" -> arity == 0 && ret.equals("net.kyori.adventure.key.Key") ? "return this.key;" : null;
            case "name" -> arity == 0 && ret.equals("java.lang.String")
                ? "return this.key.getKey().toUpperCase(java.util.Locale.ROOT).replace('.', '_').replace('/', '_');" : null;
            case "ordinal" -> arity == 0 && ret.equals("int") ? "return this.ordinal;" : null;
            case "compareTo" -> arity == 1 ? "return Integer.compare(this.ordinal, p0.ordinal());" : null;
            case "translationKey", "getTranslationKey" -> arity == 0 && ret.equals("java.lang.String") ? "return this.translationKey;" : null;
            default -> null;
        };
    }
}
